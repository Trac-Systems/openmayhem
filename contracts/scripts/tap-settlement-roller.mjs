#!/usr/bin/env node
import crypto from 'node:crypto';
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
const SESSION_RECEIPT_SCHEMA_VERSION = 8;
const SIGNING_MESSAGE_VERSION = 2;
const DEFAULT_TAP_CHALLENGE_EPOCHS = 6;
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

export const POOL_SETTLEMENT_ABI = [
  'function setRoot(bytes32 newRoot, uint256 newEpoch, uint256 newCumulativeSpent)',
  'function epoch() view returns (uint256)',
  'function cumulativeSpent() view returns (uint256)',
  'function totalDeposited() view returns (uint256)',
  'function maxEpochDelta() view returns (uint256)',
  'function merkleRoot() view returns (bytes32)',
  'function claimed(address account) view returns (uint256)',
  'function operatorClaimable() view returns (uint256)',
  'function operatorWithdrawn() view returns (uint256)',
  'function withdrawOperator(address to, uint256 amount)',
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

export function encodeWithdrawOperatorCalldata({
  to,
  amountWei,
} = {}) {
  const destination = normalizeAddress(to, 'operator fee address');
  const amount = parseBigIntString(amountWei, 'operator fee amount wei');
  if (amount <= 0n) throw new Error('operator fee amount wei must be positive');
  return POOL_SETTLEMENT_INTERFACE.encodeFunctionData('withdrawOperator', [destination, amount]);
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

function isHexBytes(value, bytes) {
  return typeof value === 'string' && new RegExp(`^[0-9a-f]{${bytes * 2}}$`, 'i').test(value);
}

function hasOwn(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key);
}

function ed25519PublicKeyFromRawHex(publicKeyHex) {
  if (!isHexBytes(publicKeyHex, 32)) throw new Error('receipt public key must be 32 bytes of hex');
  return crypto.createPublicKey({
    key: Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(publicKeyHex, 'hex')]),
    format: 'der',
    type: 'spki',
  });
}

function safeAu(value, label, { allowZero = false } = {}) {
  const raw = String(value ?? '').trim();
  if (!/^(0|[1-9]\d*)$/.test(raw)) {
    throw new Error(`${label} must be a canonical decimal au string`);
  }
  const parsed = BigInt(raw);
  if (parsed < 0n || (!allowZero && parsed === 0n)) {
    throw new Error(`${label} must be ${allowZero ? 'non-negative' : 'positive'}`);
  }
  return parsed;
}

function addWei(map, account, amountWei) {
  if (amountWei <= 0n) return;
  map.set(account, (map.get(account) ?? 0n) + amountWei);
}

function sortedDistributionEntries(map) {
  return Array.from(map.entries())
    .filter(([, amount]) => amount > 0n)
    .map(([account, amount]) => ({ account, amount }))
    .sort((a, b) => a.account.localeCompare(b.account));
}

export function canonicalReceiptBody(body) {
  const canonical = {
    schema_version: body.schema_version,
    session_id: body.session_id,
    seq: body.seq,
    final: body.final,
    rail: body.rail,
    user: body.user,
    provider: body.provider,
    enclave_id: body.enclave_id,
    model_id: body.model_id,
    price_ver: body.price_ver,
    locked_rate_map: body.locked_rate_map,
    locked_per_req_au: body.locked_per_req_au,
    locked_min_session_au: body.locked_min_session_au,
    served_ctx: body.served_ctx,
  };
  if (hasOwn(body, 'ctx_bracket')) canonical.ctx_bracket = body.ctx_bracket;
  if (hasOwn(body, 'ctx_bracket_table_ver')) canonical.ctx_bracket_table_ver = body.ctx_bracket_table_ver;
  canonical.rules_ver = body.rules_ver;
  canonical.usage = body.usage;
  canonical.au_owed_cum = body.au_owed_cum;
  canonical.prompt_hash = body.prompt_hash;
  canonical.ts = body.ts;
  return canonical;
}

export function receiptMessage(body, signingVersion = SIGNING_MESSAGE_VERSION) {
  if (signingVersion !== SIGNING_MESSAGE_VERSION) {
    throw new Error(`Unsupported signing message version: ${signingVersion}`);
  }
  return JSON.stringify({
    domain: 'mayhem-session-receipt',
    signing_version: signingVersion,
    body: canonicalReceiptBody(body),
  });
}

function verifyEd25519Hex(signatureHex, message, publicKeyHex, label) {
  if (!isHexBytes(signatureHex, 64)) throw new Error(`Invalid ${label} receipt signature`);
  const publicKey = ed25519PublicKeyFromRawHex(publicKeyHex);
  const ok = crypto.verify(null, Buffer.from(message), publicKey, Buffer.from(signatureHex, 'hex'));
  if (!ok) throw new Error(`Invalid ${label} receipt signature`);
}

export function verifyReceiptEnvelope(envelope) {
  if (!isObject(envelope?.body)) throw new Error('receipt body must be an object');
  const body = envelope.body;
  if (body.schema_version !== SESSION_RECEIPT_SCHEMA_VERSION) {
    throw new Error(`receipt schema_version must be ${SESSION_RECEIPT_SCHEMA_VERSION}`);
  }
  if (body.rail !== 'tap') throw new Error('TAP settlement receipt rail must be tap');
  for (const field of ['session_id', 'model_id', 'prompt_hash']) {
    if (typeof body[field] !== 'string' || body[field].length === 0 || body[field].length > 256) {
      throw new Error(`Invalid receipt ${field}`);
    }
  }
  if (!isHexBytes(body.user, 32)) throw new Error('Invalid receipt user public key');
  if (!isHexBytes(body.provider, 32)) throw new Error('Invalid receipt provider public key');
  if (!Number.isSafeInteger(body.seq) || body.seq < 0) throw new Error('Invalid receipt sequence');
  if (typeof body.final !== 'boolean') throw new Error('Invalid receipt final flag');
  if (!Number.isSafeInteger(body.price_ver) || body.price_ver < 1) throw new Error('Invalid receipt price version');
  if (!Number.isSafeInteger(body.rules_ver) || body.rules_ver < 1) throw new Error('Invalid receipt rules version');
  if (!Number.isSafeInteger(body.served_ctx) || body.served_ctx < 0) throw new Error('Invalid receipt served context');
  if (!Number.isSafeInteger(body.ts) || body.ts < 0) throw new Error('Invalid receipt timestamp');
  safeAu(body.locked_per_req_au, 'receipt locked_per_req_au', { allowZero: true });
  safeAu(body.locked_min_session_au, 'receipt locked_min_session_au', { allowZero: true });
  safeAu(body.au_owed_cum, 'receipt au_owed_cum');
  const enclaveKey = envelope.enclave_pubkey ?? (isHexBytes(body.enclave_id, 32) ? body.enclave_id : null);
  if (!enclaveKey) throw new Error('receipt enclave public key is required');
  const message = receiptMessage(body);
  verifyEd25519Hex(envelope.enclave_sig, message, enclaveKey, 'enclave');
  verifyEd25519Hex(envelope.user_sig, message, body.user, 'user');
  return true;
}

export function auToTapWei(au, tapUsdAu) {
  const safe = safeAu(au, 'au', { allowZero: true });
  const rate = safeAu(tapUsdAu, 'tap_usd_au');
  return (safe * TAP_WEI) / rate;
}

export function providerShareWei(spentWei, weightBps = 10_000) {
  const spent = parseBigIntString(spentWei, 'spent wei');
  const weight = parseNonNegativeInt(weightBps, 'provider weight bps');
  if (weight > 10_000) throw new Error('provider weight bps must be <= 10000');
  return (spent * PROVIDER_BPS * BigInt(weight)) / (BPS * BPS);
}

function receiptEnvelope(entry) {
  const receipt = entry.receipt ?? entry;
  const bodySource = receipt.body ?? receipt;
  const {
    enclave_sig: _enclaveSig,
    user_sig: _userSig,
    enclave_pubkey: _enclavePubkey,
    receipt_ack: _receiptAck,
    voucher: _voucher,
    ...body
  } = bodySource;
  if (!isObject(body)) throw new Error('receipt body must be an object');
  return {
    entry,
    body,
    envelope: {
      body,
      enclave_sig: receipt.enclave_sig ?? entry.enclave_sig ?? null,
      user_sig: receipt.user_sig ?? entry.user_sig ?? null,
      enclave_pubkey: receipt.enclave_pubkey ?? entry.enclave_pubkey ?? bodySource.enclave_pubkey ?? null,
    },
  };
}

function receiptDeltaAu(entry, body, previousBySession) {
  const explicit = entry.settle_au ?? entry.au_delta;
  if (explicit !== undefined) {
    throw new Error('TAP settlement amount must derive from signed receipt cumulative amount');
  }
  const current = safeAu(body.au_owed_cum, 'receipt au_owed_cum');
  const session = body.session_id;
  if (typeof session !== 'string' || session.length === 0) throw new Error('receipt session_id is required');
  const previous = safeAu(
    entry.previous_au_owed_cum ?? previousBySession.get(session) ?? '0',
    'receipt previous_au_owed_cum',
    { allowZero: true }
  );
  if (current < previous) throw new Error('receipt cumulative au regressed');
  previousBySession.set(session, current.toString());
  return current - previous;
}

function providerAccountFor(providerId, entry, body, providerAccounts) {
  const direct = entry.provider_tap_address
    ?? entry.tap_address
    ?? body.provider_tap_address
    ?? body.tap_address;
  if (direct) {
    throw new Error('provider TAP address must come from operator provider-account map');
  }
  const mapped = providerAccounts?.[providerId] ?? providerAccounts?.[providerId.toLowerCase()];
  if (mapped) {
    return normalizeAddress(mapped, `provider account for ${providerId}`);
  }
  throw new Error(`Missing TAP claim address for provider ${providerId}`);
}

function receiptProviders(entry, body, providerAccounts) {
  const refs = entry.provider_refs ?? body.provider_refs;
  if (refs !== undefined) {
    throw new Error('multi-provider TAP receipts require a signed contribution schema');
  }
  if (entry.contribution_weights_bps !== undefined || body.contribution_weights_bps !== undefined) {
    throw new Error('multi-provider TAP receipts require a signed contribution schema');
  }
  const providerId = body.provider;
  if (typeof providerId !== 'string' || providerId.length === 0) {
    throw new Error('receipt provider is required');
  }
  if (!isHexBytes(providerId, 32)) throw new Error('receipt provider must be a 32-byte public key');
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
  if (challenge <= 0) throw new Error('TAP settlement challenge_epochs must be non-zero');
  return {
    settle_through_epoch: through,
    challenge_epochs: challenge,
    holdback_epochs: holdback,
    release_delay_epochs: Math.max(challenge, holdback),
  };
}

function tapSettlementChallengeEpochs(inputBundle, explicitChallengeEpochs) {
  const raw = explicitChallengeEpochs
    ?? inputBundle?.challenge_epochs
    ?? inputBundle?.release_policy?.challenge_epochs
    ?? inputBundle?.params?.challenge_epochs
    ?? inputBundle?.audited_epoch?.params?.challenge_epochs
    ?? inputBundle?.recomputed?.params?.challenge_epochs
    ?? DEFAULT_TAP_CHALLENGE_EPOCHS;
  return parseNonNegativeInt(raw, 'TAP settlement challenge_epochs');
}

function tapSettlementHoldbackEpochs(inputBundle, explicitHoldbackEpochs) {
  const raw = explicitHoldbackEpochs
    ?? inputBundle?.holdback_epochs
    ?? inputBundle?.release_policy?.holdback_epochs
    ?? inputBundle?.params?.holdback_epochs
    ?? inputBundle?.audited_epoch?.params?.holdback_epochs
    ?? inputBundle?.recomputed?.params?.holdback_epochs
    ?? 0;
  return parseNonNegativeInt(raw, 'TAP settlement holdback_epochs', 0);
}

function tapSettlementThroughEpoch(inputBundle, explicitSettleThroughEpoch) {
  return explicitSettleThroughEpoch
    ?? inputBundle?.settle_through_epoch
    ?? inputBundle?.settleThroughEpoch
    ?? inputBundle?.release_policy?.settle_through_epoch
    ?? inputBundle?.proof_epoch
    ?? inputBundle?.settlement_epoch
    ?? null;
}

function tapLedgerFeeBps(inputBundle, explicitFeeBps) {
  const raw = explicitFeeBps
    ?? inputBundle?.params?.fee_bps
    ?? inputBundle?.audited_epoch?.params?.fee_bps
    ?? inputBundle?.recomputed?.params?.fee_bps;
  if (raw === undefined || raw === null || raw === '') {
    throw new Error('TAP settlement requires admin ledger fee_bps');
  }
  const feeBps = parseNonNegativeInt(raw, 'ledger fee_bps');
  if (feeBps !== Number(OPERATOR_BPS)) {
    throw new Error(`TAP ledger fee_bps must equal on-chain OPERATOR_BPS ${OPERATOR_BPS}`);
  }
  return feeBps;
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
    return parseBigIntString(
      entry.amount
        ?? entry.amount_wei
        ?? entry.cumulative_wei
        ?? entry.refund_wei
        ?? entry.refund,
      label
    );
  }
  return parseBigIntString(entry, label);
}

function settlementDistributionEntriesFromSource(source, label) {
  const out = new Map();
  if (Array.isArray(source)) {
    for (const entry of source) {
      const account = normalizeAddress(entry.account, `${label} account`);
      addWei(out, account, parseEntryAmount(entry, `${label} cumulative wei`));
    }
  } else if (isObject(source)) {
    for (const [account, entry] of Object.entries(source)) {
      addWei(
        out,
        normalizeAddress(account, `${label} account`),
        parseEntryAmount(entry, `${label} cumulative wei`)
      );
    }
  }
  return sortedDistributionEntries(out);
}

export function settlementDistributionEntries(report, label = 'settlement') {
  return settlementDistributionEntriesFromSource(
    report?.entries ?? report?.proofs ?? report?.providers ?? {},
    label
  );
}

function providerDistributionEntries(report, label = 'settlement providers') {
  if (report?.providers !== undefined) {
    return settlementDistributionEntriesFromSource(report.providers, label);
  }
  return settlementDistributionEntries(report, label);
}

function priorDistributionMap(prior) {
  const out = new Map();
  for (const entry of settlementDistributionEntries(prior, 'prior')) {
    out.set(entry.account, entry.amount);
  }
  return out;
}

function priorProviderMap(prior) {
  const out = new Map();
  for (const entry of providerDistributionEntries(prior, 'prior providers')) {
    out.set(entry.account, entry.amount);
  }
  return out;
}

function refundDistributionEntries(report, label = 'settlement refunds') {
  if (report?.refunds === undefined) return [];
  return settlementDistributionEntriesFromSource(report.refunds, label);
}

function priorRefundMap(prior) {
  const out = new Map();
  for (const entry of refundDistributionEntries(prior, 'prior refunds')) {
    out.set(entry.account, entry.amount);
  }
  return out;
}

function buyerRefundSource(inputBundle) {
  return inputBundle?.buyer_refunds
    ?? inputBundle?.buyerRefunds
    ?? inputBundle?.refunds
    ?? [];
}

function buyerRefundAccount(entry, buyerAccounts) {
  const direct = entry.account
    ?? entry.buyer_tap_address
    ?? entry.tap_address
    ?? entry.eth_address;
  if (direct) return normalizeAddress(direct, 'buyer refund account');
  const buyerId = entry.buyer ?? entry.user ?? entry.user_pubkey ?? entry.user_public_key;
  if (typeof buyerId !== 'string' || buyerId.length === 0) {
    throw new Error('buyer refund requires account or buyer/user id');
  }
  const mapped = buyerAccounts?.[buyerId] ?? buyerAccounts?.[buyerId.toLowerCase()];
  if (!mapped) throw new Error(`Missing TAP refund address for buyer ${buyerId}`);
  return normalizeAddress(mapped, `buyer refund account for ${buyerId}`);
}

function buyerRefundWei(entry, rate) {
  const wei = entry.cumulative_wei
    ?? entry.amount_wei
    ?? entry.refund_wei;
  if (wei !== undefined) return parseBigIntString(wei, 'buyer refund wei');
  const au = entry.cumulative_au
    ?? entry.amount_au
    ?? entry.refund_au;
  if (au !== undefined) return auToTapWei(au, rate);
  throw new Error('buyer refund requires cumulative_wei/amount_wei/refund_wei or cumulative_au/amount_au/refund_au');
}

function applyBuyerRefunds(perBuyerRefund, inputBundle, buyerAccounts, rate) {
  const source = buyerRefundSource(inputBundle);
  if (Array.isArray(source)) {
    for (const entry of source) {
      if (!isObject(entry)) throw new Error('buyer refund entries must be objects');
      addWei(perBuyerRefund, buyerRefundAccount(entry, buyerAccounts), buyerRefundWei(entry, rate));
    }
    return;
  }
  if (!isObject(source)) throw new Error('buyer refunds must be an array or account map');
  for (const [account, entry] of Object.entries(source)) {
    const amountWei = isObject(entry)
      ? buyerRefundWei({ account, ...entry }, rate)
      : parseBigIntString(entry, 'buyer refund wei');
    addWei(perBuyerRefund, normalizeAddress(account, 'buyer refund account'), amountWei);
  }
}

export function buildTapSettlement({
  bundle,
  receipts,
  providerAccounts = {},
  buyerAccounts = {},
  tapUsdAu,
  ledgerFeeBps,
  prior = null,
  settleThroughEpoch = null,
  challengeEpochs = null,
  holdbackEpochs = 0,
} = {}) {
  const inputBundle = receipts ? { receipts } : bundle;
  const input = receipts ?? normalizedReceipts(bundle);
  const rate = safeAu(tapUsdAu, 'tap_usd_au').toString();
  const feeBps = tapLedgerFeeBps(inputBundle, ledgerFeeBps);
  const policy = releasePolicy({
    settleThroughEpoch: tapSettlementThroughEpoch(inputBundle, settleThroughEpoch),
    challengeEpochs: tapSettlementChallengeEpochs(inputBundle, challengeEpochs),
    holdbackEpochs: tapSettlementHoldbackEpochs(inputBundle, holdbackEpochs),
  });
  const bundleEpoch = inputBundle?.epoch ?? inputBundle?.receipt_epoch ?? inputBundle?.settlement_epoch;
  const previousBySession = new Map();
  const perProvider = priorProviderMap(prior);
  const perBuyerRefund = priorRefundMap(prior);
  let cumulativeSpentWei = parseBigIntString(prior?.cumulative_spent_wei ?? prior?.cumulativeSpentWei, 'prior cumulative spent wei');
  let receiptCount = 0;
  let heldReceiptCount = 0;
  let spentAu = 0n;
  let heldAu = 0n;

  const sorted = input
    .map(receiptEnvelope)
    .sort((a, b) => (
      String(a.body.session_id ?? '').localeCompare(String(b.body.session_id ?? '')) ||
      Number(a.body.seq ?? 0) - Number(b.body.seq ?? 0)
    ));

  for (const { entry, body, envelope } of sorted) {
    verifyReceiptEnvelope(envelope);
    const deltaAu = receiptDeltaAu(entry, body, previousBySession);
    if (deltaAu === 0n) continue;
    const release = receiptReleaseInfo(entry, body, bundleEpoch, policy);
    if (!release.eligible) {
      heldReceiptCount += 1;
      heldAu += deltaAu;
      continue;
    }
    const spentWei = auToTapWei(deltaAu, rate);
    if (spentWei <= 0n) throw new Error('receipt converts to zero TAP wei');
    receiptCount += 1;
    spentAu += deltaAu;
    cumulativeSpentWei += spentWei;
    const providers = receiptProviders(entry, body, providerAccounts);
    let weightTotal = 0;
    for (const provider of providers) weightTotal += provider.weight_bps;
    if (weightTotal !== 10_000) throw new Error('provider weights must sum to 10000 bps');
    for (const provider of providers) {
      addWei(perProvider, provider.account, providerShareWei(spentWei, provider.weight_bps));
    }
  }

  applyBuyerRefunds(perBuyerRefund, inputBundle, buyerAccounts, rate);

  const providers = sortedDistributionEntries(perProvider);
  const refunds = sortedDistributionEntries(perBuyerRefund);
  const combined = new Map();
  for (const entry of providers) addWei(combined, entry.account, entry.amount);
  for (const entry of refunds) addWei(combined, entry.account, entry.amount);
  const entries = sortedDistributionEntries(combined);
  if (entries.length === 0) {
    return {
      posted: false,
      payout_model: 'non_custodial_claim',
      custodial_wallet: false,
      reason: heldReceiptCount > 0 ? 'no matured provider earnings' : 'no claimable provider earnings',
      tap_usd_au: rate,
      ledger_fee_bps: feeBps,
      receipt_count: receiptCount,
      held_receipt_count: heldReceiptCount,
      spent_au: spentAu.toString(),
      held_au: heldAu.toString(),
      cumulative_spent_wei: cumulativeSpentWei.toString(),
      entries: [],
      providers: [],
      refunds: [],
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
  const providerClaimedWei = providers.reduce((sum, entry) => sum + entry.amount, 0n);
  const buyerRefundWei = refunds.reduce((sum, entry) => sum + entry.amount, 0n);
  const totalClaimedWei = entries.reduce((sum, entry) => sum + entry.amount, 0n);
  const providerCapWei = (cumulativeSpentWei * PROVIDER_BPS) / BPS;
  if (providerClaimedWei > providerCapWei) {
    throw new Error('provider distribution exceeds 75% TAP settlement cap');
  }

  return {
    posted: false,
    payout_model: 'non_custodial_claim',
    custodial_wallet: false,
    tap_usd_au: rate,
    ledger_fee_bps: feeBps,
    receipt_count: receiptCount,
    held_receipt_count: heldReceiptCount,
    spent_au: spentAu.toString(),
    held_au: heldAu.toString(),
    cumulative_spent_wei: cumulativeSpentWei.toString(),
    provider_claimed_wei: providerClaimedWei.toString(),
    buyer_refund_wei: buyerRefundWei.toString(),
    total_claimed_wei: totalClaimedWei.toString(),
    provider_cap_wei: providerCapWei.toString(),
    root: dist.root,
    entries: entries.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    providers: providers.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    refunds: refunds.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
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

export async function dryRunWithdrawOperator({
  writablePool,
  to,
  amountWei,
} = {}) {
  if (!writablePool?.withdrawOperator) throw new Error('Missing writable pool contract');
  const destination = normalizeAddress(to, 'operator fee address');
  const amount = parseBigIntString(amountWei, 'operator fee amount wei');
  if (amount <= 0n) throw new Error('operator fee amount wei must be positive');
  const out = { ok: false, static_call_ok: false, gas_estimate: null };
  try {
    if (writablePool.withdrawOperator.staticCall) {
      await writablePool.withdrawOperator.staticCall(destination, amount);
      out.static_call_ok = true;
    }
    if (writablePool.withdrawOperator.estimateGas) {
      out.gas_estimate = (await writablePool.withdrawOperator.estimateGas(destination, amount)).toString();
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

async function poolRootOrNull(pool) {
  if (!pool?.merkleRoot) return null;
  try {
    return String(await pool.merkleRoot()).toLowerCase();
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
  const previousRoot = (await poolRootOrNull(pool))
    ?? (previous?.root ? String(previous.root).toLowerCase() : null)
    ?? (previous?.merkle_root ? String(previous.merkle_root).toLowerCase() : null);

  if (!Number.isSafeInteger(newEpoch) || newEpoch <= priorEpoch) reasons.push('epoch !monotonic');
  if (cumulativeSpentWei < previousSpentWei) reasons.push('spent !monotonic');
  if (cumulativeSpentWei === previousSpentWei) {
    if (previousRoot && previousRoot === String(settlement.root).toLowerCase()) {
      reasons.push('no new spend since last root');
    } else {
      flags.push('root changed without new spend');
    }
  }
  if (cumulativeSpentWei > depositedWei) reasons.push('spent > deposited');
  if (epochDeltaCapWei > 0n && cumulativeSpentWei >= previousSpentWei && cumulativeSpentWei - previousSpentWei > epochDeltaCapWei) {
    reasons.push('epoch delta > cap');
  }

  let entries = [];
  let providerEntries = [];
  try {
    entries = settlementDistributionEntries(settlement, 'settlement');
    providerEntries = providerDistributionEntries(settlement, 'settlement providers');
  } catch (error) {
    reasons.push(error?.message || 'invalid settlement entries');
  }
  const previousByAccount = priorDistributionMap(previous);
  const currentByAccount = new Map(entries.map((entry) => [entry.account, entry.amount]));
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
  for (const [account, previousAmount] of previousByAccount.entries()) {
    const currentAmount = currentByAccount.get(account);
    if (currentAmount !== undefined && currentAmount >= previousAmount) continue;
    if (!pool?.claimed) {
      reasons.push(`cumulative for ${account} dropped; on-chain claimed check required`);
      continue;
    }
    try {
      const claimedWei = parseBigIntString(await pool.claimed(account), `claimed(${account})`);
      if (claimedWei < previousAmount) {
        reasons.push(`cumulative for ${account} dropped below unclaimed prior`);
      }
    } catch (error) {
      reasons.push(`claimed(${account}) read failed: ${errorMessage(error)}`);
    }
  }

  const providerCapWei = bpsOf(cumulativeSpentWei, PROVIDER_BPS);
  const operatorCapWei = bpsOf(cumulativeSpentWei, OPERATOR_BPS);
  const burnCapWei = bpsOf(cumulativeSpentWei, BURN_BPS);
  const providerOwedWei = providerEntries.reduce((sum, entry) => sum + entry.amount, 0n);
  if (providerOwedWei > providerCapWei + PROVIDER_CAP_TOLERANCE_WEI) {
    reasons.push('provider owed > 75% spent cap');
  } else if (providerOwedWei > providerCapWei) {
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
    provider_owed_wei: providerOwedWei.toString(),
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
  buyerAccounts,
  tapUsdAu,
  ledgerFeeBps,
  prior,
  settleThroughEpoch,
  challengeEpochs,
  holdbackEpochs,
  pool,
  ownerSigner,
  operatorAddress,
  epoch,
  post = true,
} = {}) {
  const settlement = buildTapSettlement({
    bundle,
    receipts,
    providerAccounts,
    buyerAccounts,
    tapUsdAu,
    ledgerFeeBps,
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
  let operatorFee = null;
  if (pool && ownerSigner) {
    writable = pool.connect ? pool.connect(ownerSigner) : pool;
    const operatorWithdrawn = pool.operatorWithdrawn ? await pool.operatorWithdrawn() : 0n;
    const cumulativeSpentWei = parseBigIntString(settlement.cumulative_spent_wei, 'settlement cumulative spent wei');
    const operatorCapWei = bpsOf(cumulativeSpentWei, OPERATOR_BPS);
    const predictedClaimableWei = operatorCapWei > operatorWithdrawn ? operatorCapWei - operatorWithdrawn : 0n;
    const destination = operatorAddress
      ? normalizeAddress(operatorAddress, 'operator fee address')
      : null;
    operatorFee = {
      auto_sent: false,
      destination,
      predicted_claimable_wei: predictedClaimableWei.toString(),
      calldata: destination && predictedClaimableWei > 0n
        ? encodeWithdrawOperatorCalldata({ to: destination, amountWei: predictedClaimableWei })
        : null,
      dry_run: null,
      tx: null,
    };
    if (predictedClaimableWei > 0n && !destination) {
      return {
        ...settlement,
        posted: false,
        blocked: true,
        reasons: ['operator fee destination is required for TAP auto-withdraw'],
        epoch: screen.epoch,
        signing_address: signingAddress,
        set_root_calldata: setRootCalldata,
        operator_fee: operatorFee,
        screen,
      };
    }
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
        operator_fee: operatorFee,
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
    if (operatorFee?.destination) {
      const claimable = pool.operatorClaimable ? await pool.operatorClaimable() : 0n;
      operatorFee.actual_claimable_wei = claimable.toString();
      if (claimable > 0n) {
        operatorFee.calldata = encodeWithdrawOperatorCalldata({
          to: operatorFee.destination,
          amountWei: claimable,
        });
        operatorFee.dry_run = await dryRunWithdrawOperator({
          writablePool: writable,
          to: operatorFee.destination,
          amountWei: claimable,
        });
        if (!operatorFee.dry_run.ok) {
          return {
            ...settlement,
            posted: true,
            blocked: true,
            reasons: [`withdrawOperator dry-run failed: ${operatorFee.dry_run.error}`],
            epoch: screen.epoch,
            tx,
            signing_address: signingAddress,
            set_root_calldata: setRootCalldata,
            set_root_dry_run: setRootDryRun,
            operator_fee: operatorFee,
            screen,
          };
        }
        const feeSent = await writable.withdrawOperator(operatorFee.destination, claimable);
        await feeSent.wait();
        operatorFee.tx = feeSent.hash;
        operatorFee.auto_sent = true;
      }
    }
  }
  return {
    ...settlement,
    posted: Boolean(tx),
    epoch: screen.epoch,
    tx,
    signing_address: signingAddress,
    set_root_calldata: setRootCalldata,
    set_root_dry_run: setRootDryRun,
    operator_fee: operatorFee,
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

async function resolveTapUsdAu({ tapUsdAu, peerRpcUrl, fetchImpl } = {}) {
  if (tapUsdAu !== undefined && tapUsdAu !== null && tapUsdAu !== '') {
    return safeAu(tapUsdAu, '--tap-usd-au').toString();
  }
  const rate = await readContractStateValue(peerRpcUrl, 'tap/rate/latest', { fetchImpl });
  return safeAu(rate?.tap_usd_au, 'tap/rate/latest.tap_usd_au').toString();
}

function buildReplayCommand({
  bundlePath,
  providerAccountsPath,
  buyerAccountsPath,
  tapUsdAu,
  ledgerFeeBps,
  priorPath,
  settleThroughEpoch,
  challengeEpochs,
  holdbackEpochs,
  ethRpc,
  poolAddress,
  operatorAddress,
  epoch,
  confirm,
  json,
} = {}) {
  const args = ['node', 'contracts/scripts/tap-settlement-roller.mjs'];
  if (bundlePath) args.push('--bundle', bundlePath);
  if (providerAccountsPath) args.push('--provider-accounts', providerAccountsPath);
  if (buyerAccountsPath) args.push('--buyer-accounts', buyerAccountsPath);
  if (tapUsdAu) args.push('--tap-usd-au', String(tapUsdAu));
  if (ledgerFeeBps !== undefined && ledgerFeeBps !== null) args.push('--ledger-fee-bps', String(ledgerFeeBps));
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
  if (operatorAddress) args.push('--operator-address', operatorAddress);
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
  const buyerAccountsPath = args['buyer-accounts'];
  const buyerAccounts = buyerAccountsPath
    ? readJson(path.resolve(buyerAccountsPath), 'buyer accounts')
    : {};
  const priorPath = args.prior;
  const prior = priorPath ? readJson(path.resolve(priorPath), 'prior settlement') : null;
  const peerRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const tapUsdAu = await resolveTapUsdAu({
    tapUsdAu: args['tap-usd-au'],
    peerRpcUrl,
  });
  const ledgerFeeBps = args['ledger-fee-bps'];

  const ethRpc = args['eth-rpc'] || args.rpc || process.env.MAYHEM_TAP_ETH_RPC;
  const poolAddress = args.pool || process.env.MAYHEM_TAP_POOL_ADDRESS;
  const operatorAddress = args['operator-address'] || process.env.MAYHEM_TAP_OPERATOR_ADDRESS;
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
    buyerAccounts,
    prior,
    tapUsdAu,
    ledgerFeeBps,
    settleThroughEpoch: args['settle-through-epoch'],
    challengeEpochs: args['challenge-epochs'],
    holdbackEpochs: args['holdback-epochs'] ?? 0,
    pool,
    ownerSigner,
    operatorAddress,
    epoch: args.epoch ? parsePositiveInt(args.epoch, '--epoch') : undefined,
    post: confirm,
  });
  if (signerEnvName) report.signer_env = signerEnvName;
  report.copy_paste_replay_command = buildReplayCommand({
    bundlePath,
    providerAccountsPath,
    buyerAccountsPath,
    tapUsdAu,
    ledgerFeeBps,
    priorPath,
    settleThroughEpoch: args['settle-through-epoch'],
    challengeEpochs: args['challenge-epochs'],
    holdbackEpochs: args['holdback-epochs'],
    ethRpc,
    poolAddress,
    operatorAddress,
    epoch: report.epoch,
    confirm,
    json: true,
  });
  if (!confirm && report.root && ethRpc && poolAddress) {
    report.copy_paste_confirm_command = buildReplayCommand({
      bundlePath,
      providerAccountsPath,
      buyerAccountsPath,
      tapUsdAu,
      ledgerFeeBps,
      priorPath,
      settleThroughEpoch: args['settle-through-epoch'],
      challengeEpochs: args['challenge-epochs'],
      holdbackEpochs: args['holdback-epochs'],
      ethRpc,
      poolAddress,
      operatorAddress,
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
