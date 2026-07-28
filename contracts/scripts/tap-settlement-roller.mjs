#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { distribution } from './merkle.mjs';
import { signRootProposal } from './pool-governance.mjs';
import { safeErrorMessage } from './safe-output.mjs';
import {
  TAP_GOVERNANCE_SIGNER_ENV,
  TAP_ROLLER_SIGNER_ENV,
  walletFromEnv,
} from './signer-env.mjs';

const scriptPath = fileURLToPath(import.meta.url);
let cliEthProvider = null;
export const TAP_WEI = 1_000_000_000_000_000_000n;
export const BPS = 10_000n;
export const PROVIDER_BPS = 7_500n;
export const OPERATOR_BPS = 1_500n;
const PROVIDER_CAP_TOLERANCE_WEI = 0n;
const SESSION_RECEIPT_SCHEMA_VERSION = 10;
const SIGNING_MESSAGE_VERSION = 2;
const DEFAULT_TAP_CHALLENGE_EPOCHS = 6;
const CONTRACT_VERSION = 18;
const TAP_BURN_BPS = 1_000n;
const MIN_TAP_CONFIRMATION_DEPTH = 12;
const PREPARATION_CONFIRM_TIMEOUT_MS = 30_000;
const PREPARATION_CONFIRM_INTERVAL_MS = 250;
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');
let preparationDependenciesPromise = null;

export const POOL_SETTLEMENT_ABI = [
  'event RootProposed(uint256 indexed epoch, bytes32 merkleRoot, uint256 cumulativeSpent, uint256 indexed nonce, uint64 executeAfter)',
  'event RootPosted(uint256 indexed epoch, bytes32 merkleRoot, uint256 cumulativeSpent, uint256 indexed nonce)',
  'function token() view returns (address)',
  'function proposeRoot(bytes32 newRoot, uint256 newEpoch, uint256 newCumulativeSpent, bytes governanceSignature)',
  'function executeRoot()',
  'function governanceNonce() view returns (uint256)',
  'function governanceDelay() view returns (uint64)',
  'function governanceSigner() view returns (address)',
  'function pendingRoot() view returns (bytes32 merkleRoot, uint256 newEpoch, uint256 newCumulativeSpent, uint256 previousEpoch, uint256 previousCumulativeSpent, uint256 nonce, uint64 executeAfter)',
  'function epoch() view returns (uint256)',
  'function cumulativeSpent() view returns (uint256)',
  'function totalDeposited() view returns (uint256)',
  'function maxEpochDelta() view returns (uint256)',
  'function merkleRoot() view returns (bytes32)',
  'function merkleRootAtEpoch(uint256 rootEpoch) view returns (bytes32)',
  'function claimed(address account) view returns (uint256)',
  'function operatorClaimable() view returns (uint256)',
  'function operatorWithdrawn() view returns (uint256)',
  'function withdrawOperator(address to, uint256 amount)',
  'function burnClaimable() view returns (uint256)',
  'function totalBurned() view returns (uint256)',
  'function burn() returns (uint256 amount)',
];
const POOL_SETTLEMENT_INTERFACE = new ethers.Interface(POOL_SETTLEMENT_ABI);
export const TAP_ROLLER_SIGNER_ENVS = [TAP_ROLLER_SIGNER_ENV];
export const TAP_GOVERNANCE_SIGNER_ENVS = [TAP_GOVERNANCE_SIGNER_ENV];

export function tapRollerWallet(provider, env = process.env) {
  return walletFromEnv(provider, {
    env,
    names: TAP_ROLLER_SIGNER_ENVS,
    label: 'TAP settlement roller private key',
  });
}

export function tapGovernanceWallet(provider, env = process.env) {
  return walletFromEnv(provider, {
    env,
    names: TAP_GOVERNANCE_SIGNER_ENVS,
    label: 'independent TAP governance private key',
  });
}

function errorMessage(error) {
  return safeErrorMessage(error);
}

export function encodeProposeRootCalldata({
  root,
  epoch,
  cumulativeSpentWei,
  governanceSignature,
} = {}) {
  if (!root) throw new Error('Missing proposeRoot root');
  const safeEpoch = parseNonNegativeInt(epoch, 'proposeRoot epoch');
  const spent = parseBigIntString(cumulativeSpentWei, 'proposeRoot cumulative spent wei');
  if (!/^0x[0-9a-fA-F]+$/.test(String(governanceSignature ?? ''))) {
    throw new Error('Missing proposeRoot governance signature');
  }
  return POOL_SETTLEMENT_INTERFACE.encodeFunctionData('proposeRoot', [
    root,
    BigInt(safeEpoch),
    spent,
    governanceSignature,
  ]);
}

export function encodeExecuteRootCalldata() {
  return POOL_SETTLEMENT_INTERFACE.encodeFunctionData('executeRoot');
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

export function encodeBurnCalldata() {
  return POOL_SETTLEMENT_INTERFACE.encodeFunctionData('burn');
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
    billing_id: body.billing_id,
    billing_attempt: body.billing_attempt,
    billing_prior_usage: body.billing_prior_usage,
    billing_prior_au_owed_cum: body.billing_prior_au_owed_cum,
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
  if (isObject(body.usage_attribution) && Object.keys(body.usage_attribution).length > 0) {
    canonical.usage_attribution = body.usage_attribution;
  }
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
  if (!isHexBytes(body.billing_id, 32) || body.billing_id !== body.billing_id.toLowerCase()) {
    throw new Error('Invalid receipt billing id');
  }
  if (!Number.isSafeInteger(body.billing_attempt) || body.billing_attempt < 0) {
    throw new Error('Invalid receipt billing attempt');
  }
  const billingPriorUsage = normalizeReceiptUsage(body.billing_prior_usage);
  if (stableJson(billingPriorUsage) !== stableJson(body.billing_prior_usage)) {
    throw new Error('receipt billing_prior_usage must be canonical');
  }
  const usage = normalizeReceiptUsage(body.usage);
  if (stableJson(usage) !== stableJson(body.usage)) {
    throw new Error('receipt usage must be canonical');
  }
  const billingPriorAu = safeAu(
    body.billing_prior_au_owed_cum,
    'receipt billing_prior_au_owed_cum',
    { allowZero: true }
  );
  if (
    body.billing_attempt === 0 &&
    (Object.keys(billingPriorUsage).length > 0 || billingPriorAu !== 0n)
  ) {
    throw new Error('initial receipt billing attempt must have an empty baseline');
  }
  if (billingPriorAu === 0n && Object.keys(billingPriorUsage).length > 0) {
    throw new Error('receipt billing prior usage requires a prior cumulative amount');
  }
  assertUsageMonotonic(billingPriorUsage, usage);
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
  const currentAu = safeAu(body.au_owed_cum, 'receipt au_owed_cum');
  if (currentAu < billingPriorAu) {
    throw new Error('receipt cumulative au regressed below its signed billing baseline');
  }
  const enclaveKey = envelope.enclave_pubkey ?? (isHexBytes(body.enclave_id, 32) ? body.enclave_id : null);
  if (!enclaveKey) throw new Error('receipt enclave public key is required');
  const message = receiptMessage(body);
  verifyEd25519Hex(envelope.enclave_sig, message, enclaveKey, 'enclave');
  verifyEd25519Hex(envelope.user_sig, message, body.user, 'user');
  return true;
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

function normalizeReceiptUsage(source) {
  if (!isObject(source)) throw new Error('receipt usage must be an object');
  const usage = {};
  for (const [rawUnit, count] of Object.entries(source)) {
    if (typeof rawUnit !== 'string' || !/^[a-zA-Z0-9._:-]{1,128}$/.test(rawUnit)) {
      throw new Error('receipt usage unit is invalid');
    }
    if (!Number.isSafeInteger(count) || count < 0) {
      throw new Error('receipt usage count must be a non-negative safe integer');
    }
    if (count === 0) continue;
    const unit = canonicalUsageUnit(rawUnit);
    const next = (usage[unit] ?? 0) + count;
    if (!Number.isSafeInteger(next)) throw new Error('receipt usage count overflow');
    usage[unit] = next;
  }
  return Object.fromEntries(Object.entries(usage).sort(([left], [right]) => left.localeCompare(right)));
}

function assertUsageMonotonic(previous, current) {
  for (const [unit, previousCount] of Object.entries(previous)) {
    if ((current[unit] ?? 0) < previousCount) {
      throw new Error(`receipt cumulative usage regressed for ${unit}`);
    }
  }
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

function receiptDeltaAu(entry, body, billingStates) {
  const explicit = entry.settle_au ?? entry.au_delta;
  if (explicit !== undefined) {
    throw new Error('TAP settlement amount must derive from signed receipt cumulative amount');
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

function rejectReceiptPayoutAddress(entry, body) {
  const direct = entry.provider_tap_address
    ?? entry.tap_address
    ?? body.provider_tap_address
    ?? body.tap_address;
  if (direct) {
    throw new Error('provider TAP address must come from an immutable targeted payout binding');
  }
}

export function targetedTapSessionBindingKey({ epoch, user, sessionId } = {}) {
  const safeEpoch = parsePositiveInt(epoch, 'targeted TAP allocation epoch');
  if (!isHexBytes(user, 32)) throw new Error('targeted TAP allocation user must be 32 bytes of hex');
  if (typeof sessionId !== 'string' || sessionId.length === 0 || sessionId.length > 256) {
    throw new Error('targeted TAP allocation session id is invalid');
  }
  return `${safeEpoch}/${user.toLowerCase()}/${sessionId}`;
}

function targetedBindingForReceipt(entry, body, bundleEpoch, targetedSessionBindings) {
  const epoch = entryEpoch(entry, body, bundleEpoch);
  const key = targetedTapSessionBindingKey({
    epoch,
    user: body.user,
    sessionId: body.session_id,
  });
  const binding = targetedSessionBindings[key];
  if (!isObject(binding)) {
    throw new Error(`Missing targeted TAP payout allocation for session ${body.session_id}`);
  }
  if (binding.provider !== body.provider ||
      !isHexBytes(binding.payout_revision, 32)) {
    throw new Error('targeted TAP payout allocation does not match receipt provider');
  }
  safeAu(binding.au, 'targeted TAP allocation au');
  return { key, ...binding };
}

function receiptProviders(entry, body, targetedBinding) {
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
  rejectReceiptPayoutAddress(entry, body);
  return [{
    provider_id: providerId,
    account: normalizeAddress(
      targetedBinding.account,
      `targeted provider account for ${providerId}`
    ),
    payout_revision: targetedBinding.payout_revision,
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

function tapPayoutMinAu(inputBundle) {
  if (hasOwn(inputBundle ?? {}, 'payout_min_au')) {
    throw new Error(
      'top-level payout_min_au is not accepted; use params.payout_min_au from the frozen admin-ledger bundle'
    );
  }
  const sources = [
    inputBundle?.params?.payout_min_au,
    inputBundle?.audited_epoch?.params?.payout_min_au,
    inputBundle?.recomputed?.params?.payout_min_au,
  ].filter((value) => value !== undefined && value !== null && value !== '');
  if (sources.length === 0) {
    throw new Error('TAP settlement requires frozen admin-ledger params.payout_min_au');
  }
  const minimums = sources.map((value) => {
    if (typeof value !== 'string') {
      throw new Error('TAP payout_min_au must be a canonical decimal au string');
    }
    return safeAu(value, 'TAP payout_min_au', { allowZero: true });
  });
  if (minimums.some((value) => value !== minimums[0])) {
    throw new Error('TAP payout_min_au disagrees across frozen settlement evidence');
  }
  return minimums[0];
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

function tapLiabilityIdentity(provider, payoutRevision, target) {
  return `${provider.toLowerCase()}/${payoutRevision.toLowerCase()}/${target.toLowerCase()}`;
}

function canonicalTapLiabilityMap(source) {
  if (!Array.isArray(source)) {
    throw new Error('confirmed canonical TAP liabilities are required');
  }
  const out = new Map();
  for (const entry of source) {
    if (!isObject(entry) ||
        !isHexBytes(entry.provider, 32) ||
        !isHexBytes(entry.payout_revision ?? entry.revision, 32) ||
        entry.rail !== 'tap') {
      throw new Error('canonical TAP payout liability identity is invalid');
    }
    const provider = entry.provider.toLowerCase();
    const payoutRevision = String(entry.payout_revision ?? entry.revision).toLowerCase();
    const target = normalizeAddress(entry.target, 'canonical TAP payout liability target');
    const totalAu = safeAu(entry.total_au, 'canonical TAP liability total_au', {
      allowZero: true,
    });
    const heldAu = safeAu(entry.held_au, 'canonical TAP liability held_au', {
      allowZero: true,
    });
    const paidCumAu = safeAu(entry.paid_cum_au, 'canonical TAP liability paid_cum_au', {
      allowZero: true,
    });
    const aggregatePaidCumAu = safeAu(
      entry.aggregate_paid_cum_au,
      'canonical TAP aggregate paid_cum_au',
      { allowZero: true }
    );
    if (heldAu > totalAu || paidCumAu > totalAu - heldAu) {
      throw new Error('canonical TAP payout liability violates AU conservation');
    }
    const key = tapLiabilityIdentity(provider, payoutRevision, target);
    if (out.has(key)) {
      throw new Error('canonical TAP payout liability identity is duplicated');
    }
    out.set(key, {
      provider,
      payout_revision: payoutRevision,
      target,
      chain_id: parsePositiveInt(entry.chain_id, 'canonical TAP liability chain_id'),
      total_au: totalAu,
      held_au: heldAu,
      paid_cum_au: paidCumAu,
      aggregate_paid_cum_au: aggregatePaidCumAu,
      payable_au: totalAu - heldAu - paidCumAu,
      updated_epoch: parseNonNegativeInt(
        entry.updated_epoch,
        'canonical TAP liability updated_epoch',
        0
      ),
      updated_at: entry.updated_at ?? null,
    });
  }
  return out;
}

function sortedTapLiabilities(map) {
  return [...map.values()].sort((left, right) => (
    left.provider.localeCompare(right.provider) ||
    left.payout_revision.localeCompare(right.payout_revision) ||
    left.target.localeCompare(right.target)
  ));
}

function ceilDiv(value, divisor) {
  return value === 0n ? 0n : (value + divisor - 1n) / divisor;
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

function validatePriorSettlementState(prior) {
  if (!prior?.root) return;
  const expectedRoot = String(prior.root).toLowerCase();
  if (!/^0x[0-9a-f]{64}$/.test(expectedRoot)) {
    throw new Error('prior TAP settlement root is invalid');
  }
  const entries = settlementDistributionEntries(prior, 'prior settlement');
  if (entries.length === 0 || distribution(entries).root.toLowerCase() !== expectedRoot) {
    throw new Error('prior TAP settlement entries do not match the confirmed root');
  }
  const providers = providerDistributionEntries(prior, 'prior providers');
  const refunds = refundDistributionEntries(prior, 'prior refunds');
  const recombined = new Map();
  for (const entry of providers) addWei(recombined, entry.account, entry.amount);
  for (const entry of refunds) addWei(recombined, entry.account, entry.amount);
  const recombinedEntries = sortedDistributionEntries(recombined);
  if (stableJson(
    recombinedEntries.map((entry) => ({
      account: entry.account,
      cumulative_wei: entry.amount.toString(),
    }))
  ) !== stableJson(entries.map((entry) => ({
    account: entry.account,
    cumulative_wei: entry.amount.toString(),
  })))) {
    throw new Error('prior TAP provider/refund distributions do not match the confirmed root');
  }
  const cumulativeSpentWei = parseBigIntString(
    prior.cumulative_spent_wei ?? prior.cumulativeSpentWei,
    'prior cumulative spent wei'
  );
  const providerClaimedWei = providers.reduce((sum, entry) => sum + entry.amount, 0n);
  if (providerClaimedWei !== bpsOf(cumulativeSpentWei, PROVIDER_BPS)) {
    throw new Error('prior TAP provider distribution does not match its exact provider cap');
  }
}

export function buildTapSettlement({
  bundle,
  receipts,
  buyerAccounts = {},
  tapUsdAu,
  ledgerFeeBps,
  prior = null,
  settleThroughEpoch = null,
  challengeEpochs = null,
  holdbackEpochs = 0,
  targetedSessionBindings,
  canonicalLiabilities,
} = {}) {
  const inputBundle = receipts ? { receipts } : bundle;
  const input = receipts ?? normalizedReceipts(bundle);
  validatePriorSettlementState(prior);
  if (!isObject(targetedSessionBindings)) {
    throw new Error('confirmed targeted TAP session bindings are required');
  }
  const rate = safeAu(tapUsdAu, 'tap_usd_au').toString();
  const feeBps = tapLedgerFeeBps(inputBundle, ledgerFeeBps);
  const payoutMinimumAu = tapPayoutMinAu(inputBundle);
  const policy = releasePolicy({
    settleThroughEpoch: tapSettlementThroughEpoch(inputBundle, settleThroughEpoch),
    challengeEpochs: tapSettlementChallengeEpochs(inputBundle, challengeEpochs),
    holdbackEpochs: tapSettlementHoldbackEpochs(inputBundle, holdbackEpochs),
  });
  const bundleEpoch = inputBundle?.epoch ?? inputBundle?.receipt_epoch ?? inputBundle?.settlement_epoch;
  const billingStates = new Map();
  const perProvider = priorProviderMap(prior);
  const priorProviderClaimedWei = [...perProvider.values()]
    .reduce((sum, amount) => sum + amount, 0n);
  const liabilityMap = canonicalTapLiabilityMap(canonicalLiabilities);
  const perBuyerRefund = priorRefundMap(prior);
  let cumulativeSpentWei = parseBigIntString(prior?.cumulative_spent_wei ?? prior?.cumulativeSpentWei, 'prior cumulative spent wei');
  let receiptCount = 0;
  let heldReceiptCount = 0;
  let spentAu = 0n;
  let heldAu = 0n;
  let thresholdHeldAu = 0n;
  let thresholdHeldProviderCount = 0;
  const targetedAllocatedAu = new Map();
  const targetedBindingsUsed = new Map();
  const requiredLiabilityKeys = new Set();
  const checkpointOutputs = [];
  const deferredLiabilities = [];
  const aggregatePaidCursors = new Map();

  const sorted = input
    .map(receiptEnvelope)
    .sort((a, b) => (
      String(a.body.billing_id ?? '').localeCompare(String(b.body.billing_id ?? '')) ||
      Number(a.body.billing_attempt ?? 0) - Number(b.body.billing_attempt ?? 0) ||
      Number(a.body.seq ?? 0) - Number(b.body.seq ?? 0)
    ));

  for (const { entry, body, envelope } of sorted) {
    verifyReceiptEnvelope(envelope);
    const deltaAu = receiptDeltaAu(entry, body, billingStates);
    if (deltaAu === 0n) continue;
    const targetedBinding = targetedBindingForReceipt(
      entry,
      body,
      bundleEpoch,
      targetedSessionBindings
    );
    const nextAllocated = (targetedAllocatedAu.get(targetedBinding.key) ?? 0n) + deltaAu;
    const expectedAllocated = safeAu(
      targetedBinding.au,
      'targeted TAP allocation au'
    );
    if (nextAllocated > expectedAllocated) {
      throw new Error('TAP receipt amount exceeds targeted session allocation');
    }
    targetedAllocatedAu.set(targetedBinding.key, nextAllocated);
    targetedBindingsUsed.set(targetedBinding.key, targetedBinding);
    receiptCount += 1;
    const providers = receiptProviders(entry, body, targetedBinding);
    let weightTotal = 0;
    for (const provider of providers) weightTotal += provider.weight_bps;
    if (weightTotal !== 10_000) throw new Error('provider weights must sum to 10000 bps');
    for (const provider of providers) {
      requiredLiabilityKeys.add(tapLiabilityIdentity(
        provider.provider_id,
        provider.payout_revision,
        provider.account
      ));
    }
  }
  for (const [key, binding] of targetedBindingsUsed) {
    if (targetedAllocatedAu.get(key) !== safeAu(binding.au, 'targeted TAP allocation au')) {
      throw new Error('TAP receipt amount does not equal targeted session allocation');
    }
  }

  for (const key of requiredLiabilityKeys) {
    if (!liabilityMap.has(key)) {
      throw new Error(`confirmed canonical TAP liability is missing for ${key}`);
    }
  }
  for (const liability of sortedTapLiabilities(new Map(
    [...liabilityMap].filter(([key]) => requiredLiabilityKeys.has(key))
  ))) {
    if (liability.held_au > 0n) {
      heldReceiptCount += 1;
      heldAu += liability.held_au;
    }
    if (liability.payable_au === 0n) continue;
    const providerWei = auToTapWei(liability.payable_au, rate);
    if (liability.payable_au < payoutMinimumAu || providerWei === 0n) {
      thresholdHeldProviderCount += 1;
      thresholdHeldAu += liability.payable_au;
      deferredLiabilities.push({
        provider: liability.provider,
        payout_revision: liability.payout_revision,
        to: liability.target,
        payable_au: liability.payable_au.toString(),
        reason: liability.payable_au < payoutMinimumAu
          ? 'below_payout_minimum'
          : 'below_tap_wei_precision',
      });
      continue;
    }
    const previousCumulativeWei = perProvider.get(liability.target) ?? 0n;
    const newCumulativeWei = previousCumulativeWei + providerWei;
    const aggregatePaidCumAuBefore = aggregatePaidCursors.get(liability.provider)
      ?? liability.aggregate_paid_cum_au;
    if (aggregatePaidCumAuBefore < liability.aggregate_paid_cum_au) {
      throw new Error('canonical TAP aggregate paid watermark regressed');
    }
    perProvider.set(liability.target, newCumulativeWei);
    aggregatePaidCursors.set(
      liability.provider,
      aggregatePaidCumAuBefore + liability.payable_au
    );
    spentAu += liability.payable_au;
    checkpointOutputs.push({
      role: 'provider',
      provider: liability.provider,
      payout_revision: liability.payout_revision,
      to: liability.target,
      liability_total_au: liability.total_au.toString(),
      held_au: liability.held_au.toString(),
      paid_cum_au_before: liability.paid_cum_au.toString(),
      aggregate_paid_cum_au_before: aggregatePaidCumAuBefore.toString(),
      net_au_paid: liability.payable_au.toString(),
      tap_wei: providerWei.toString(),
      previous_cumulative_wei: previousCumulativeWei.toString(),
      new_cumulative_wei: newCumulativeWei.toString(),
    });
  }

  applyBuyerRefunds(perBuyerRefund, inputBundle, buyerAccounts, rate);

  const providers = sortedDistributionEntries(perProvider);
  const providerClaimedWei = providers.reduce((sum, entry) => sum + entry.amount, 0n);
  if (priorProviderClaimedWei > providerClaimedWei) {
    throw new Error('provider cumulative TAP claims regressed');
  }
  if (providerClaimedWei > priorProviderClaimedWei) {
    const requiredCumulativeSpentWei = ceilDiv(
      providerClaimedWei * BPS,
      PROVIDER_BPS
    );
    if (requiredCumulativeSpentWei < cumulativeSpentWei) {
      throw new Error('canonical TAP liability payout conflicts with prior cumulative spend');
    }
    cumulativeSpentWei = requiredCumulativeSpentWei;
  }
  const providerCapWei = (cumulativeSpentWei * PROVIDER_BPS) / BPS;
  if (providerClaimedWei !== providerCapWei) {
    throw new Error('provider distribution does not exactly match the TAP provider cap');
  }
  const refunds = sortedDistributionEntries(perBuyerRefund);
  const combined = new Map();
  for (const entry of providers) addWei(combined, entry.account, entry.amount);
  for (const entry of refunds) addWei(combined, entry.account, entry.amount);
  const entries = sortedDistributionEntries(combined);
  if (entries.length === 0) {
    let reason = 'no claimable provider earnings';
    if (heldReceiptCount > 0) reason = 'provider earnings await challenge or holdback maturity';
    else if (thresholdHeldProviderCount > 0) reason = 'provider earnings are below payout minimum';
    return {
      posted: false,
      payout_model: 'non_custodial_claim',
      custodial_wallet: false,
      reason,
      tap_usd_au: rate,
      ledger_fee_bps: feeBps,
      receipt_count: receiptCount,
      held_receipt_count: heldReceiptCount,
      threshold_held_provider_count: thresholdHeldProviderCount,
      spent_au: spentAu.toString(),
      held_au: (heldAu + thresholdHeldAu).toString(),
      threshold_held_au: thresholdHeldAu.toString(),
      cumulative_spent_wei: cumulativeSpentWei.toString(),
      payout_min_au: payoutMinimumAu.toString(),
      canonical_deferred_liabilities: deferredLiabilities,
      checkpoint_outputs: checkpointOutputs,
      entries: [],
      providers: [],
      refunds: [],
      payout_bindings: Array.from(targetedBindingsUsed.values())
        .map(({ key: _key, ...binding }) => binding)
        .sort((left, right) => (
          left.epoch - right.epoch ||
          left.provider.localeCompare(right.provider) ||
          left.session_id.localeCompare(right.session_id)
        )),
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
  const buyerRefundWei = refunds.reduce((sum, entry) => sum + entry.amount, 0n);
  const totalClaimedWei = entries.reduce((sum, entry) => sum + entry.amount, 0n);

  return {
    posted: false,
    payout_model: 'non_custodial_claim',
    custodial_wallet: false,
    tap_usd_au: rate,
    ledger_fee_bps: feeBps,
    receipt_count: receiptCount,
    held_receipt_count: heldReceiptCount,
    threshold_held_provider_count: thresholdHeldProviderCount,
    spent_au: spentAu.toString(),
    held_au: (heldAu + thresholdHeldAu).toString(),
    threshold_held_au: thresholdHeldAu.toString(),
    cumulative_spent_wei: cumulativeSpentWei.toString(),
    payout_min_au: payoutMinimumAu.toString(),
    canonical_deferred_liabilities: deferredLiabilities,
    checkpoint_outputs: checkpointOutputs,
    provider_claimed_wei: providerClaimedWei.toString(),
    buyer_refund_wei: buyerRefundWei.toString(),
    total_claimed_wei: totalClaimedWei.toString(),
    provider_cap_wei: providerCapWei.toString(),
    provider_dust_wei: '0',
    provider_dust_recipient: null,
    root: dist.root,
    entries: entries.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    providers: providers.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    refunds: refunds.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    payout_bindings: Array.from(targetedBindingsUsed.values())
      .map(({ key: _key, ...binding }) => binding)
      .sort((left, right) => (
        left.epoch - right.epoch ||
        left.provider.localeCompare(right.provider) ||
        left.session_id.localeCompare(right.session_id)
      )),
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

export async function dryRunProposeRoot({
  writablePool,
  root,
  epoch,
  cumulativeSpentWei,
  governanceSignature,
} = {}) {
  if (!writablePool?.proposeRoot) throw new Error('Missing writable pool contract');
  const safeEpoch = parseNonNegativeInt(epoch, 'proposeRoot epoch');
  const spent = parseBigIntString(cumulativeSpentWei, 'proposeRoot cumulative spent wei');
  const out = { ok: false, static_call_ok: false, gas_estimate: null };
  try {
    if (writablePool.proposeRoot.staticCall) {
      await writablePool.proposeRoot.staticCall(root, safeEpoch, spent, governanceSignature);
      out.static_call_ok = true;
    }
    if (writablePool.proposeRoot.estimateGas) {
      out.gas_estimate = (
        await writablePool.proposeRoot.estimateGas(root, safeEpoch, spent, governanceSignature)
      ).toString();
    }
    out.ok = true;
    return out;
  } catch (error) {
    return { ...out, error: errorMessage(error) };
  }
}

export async function dryRunExecuteRoot({ writablePool } = {}) {
  if (!writablePool?.executeRoot) throw new Error('Missing writable pool contract');
  const out = { ok: false, static_call_ok: false, gas_estimate: null };
  try {
    if (writablePool.executeRoot.staticCall) {
      await writablePool.executeRoot.staticCall();
      out.static_call_ok = true;
    }
    if (writablePool.executeRoot.estimateGas) {
      out.gas_estimate = (await writablePool.executeRoot.estimateGas()).toString();
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

export async function dryRunBurn({ writablePool } = {}) {
  if (!writablePool?.burn) throw new Error('Missing writable pool contract');
  const out = { ok: false, static_call_ok: false, gas_estimate: null };
  try {
    if (writablePool.burn.staticCall) {
      out.amount_wei = (await writablePool.burn.staticCall()).toString();
      out.static_call_ok = true;
    }
    if (writablePool.burn.estimateGas) {
      out.gas_estimate = (await writablePool.burn.estimateGas()).toString();
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
  const burnCapWei = cumulativeSpentWei - providerCapWei - operatorCapWei;
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

function pendingRootRecord(value) {
  const executeAfter = BigInt(value?.executeAfter ?? value?.[6] ?? 0);
  if (executeAfter === 0n) return null;
  return {
    merkle_root: String(value?.merkleRoot ?? value?.[0]).toLowerCase(),
    epoch: Number(value?.newEpoch ?? value?.[1]),
    cumulative_spent_wei: BigInt(value?.newCumulativeSpent ?? value?.[2]).toString(),
    previous_epoch: Number(value?.previousEpoch ?? value?.[3]),
    previous_cumulative_spent_wei: BigInt(value?.previousCumulativeSpent ?? value?.[4]).toString(),
    nonce: BigInt(value?.nonce ?? value?.[5]).toString(),
    execute_after: Number(executeAfter),
  };
}

async function poolTimestamp(pool, blockTag = 'latest') {
  const provider = pool?.runner?.provider;
  if (!provider) throw new Error('Pool contract has no provider');
  const block = await provider.getBlock(blockTag);
  if (!block || !Number.isSafeInteger(Number(block.timestamp))) {
    throw new Error('Could not read latest Ethereum block timestamp');
  }
  return Number(block.timestamp);
}

function matchingTapCheckpoint(checkpoint, {
  epoch,
  root,
  cumulativeSpentWei,
  epochApplyHash,
  tapRateLock,
} = {}) {
  return isObject(checkpoint) &&
    checkpoint.op === 'settle_targeted_tap' &&
    checkpoint.rail === 'tap' &&
    checkpoint.epoch === epoch &&
    checkpoint.epoch_apply_hash === epochApplyHash &&
    String(checkpoint.root ?? '').toLowerCase() === root.toLowerCase() &&
    checkpoint.cumulative_spent_wei === cumulativeSpentWei.toString() &&
    checkpoint.tap_rate_lock?.rate_record_key === tapRateLock.rate_record_key &&
    checkpoint.tap_rate_lock?.tap_usd_au === tapRateLock.tap_usd_au;
}

function checkpointReplaySettlement({
  prior,
  epoch,
  epochApplyHash,
  tapRateLock,
  canonicalLiabilities,
} = {}) {
  const checkpoint = prior?.tap_settlement_checkpoint;
  if (prior?.awaiting_finality === true &&
      prior.root_confirmed === true &&
      prior.epoch === epoch &&
      prior.epoch_apply_hash === epochApplyHash.toLowerCase() &&
      prior.tap_rate_lock?.rate_record_key === tapRateLock?.rate_record_key &&
      prior.tap_rate_lock?.tap_usd_au === tapRateLock?.tap_usd_au) {
    validatePriorSettlementState(prior);
    canonicalTapLiabilityMap(canonicalLiabilities);
    return prior;
  }
  if (!isObject(checkpoint) ||
      !isObject(tapRateLock) ||
      !isHexBytes(epochApplyHash, 32) ||
      checkpoint.epoch !== epoch ||
      checkpoint.epoch_apply_hash !== epochApplyHash.toLowerCase() ||
      checkpoint.tap_rate_lock?.rate_record_key !== tapRateLock.rate_record_key ||
      checkpoint.tap_rate_lock?.tap_usd_au !== tapRateLock.tap_usd_au) {
    return null;
  }
  if (checkpoint.root_confirmed !== true ||
      String(prior.root ?? '').toLowerCase() !== checkpoint.root ||
      String(prior.cumulative_spent_wei ?? '') !== checkpoint.cumulative_spent_wei) {
    throw new Error('prior TAP checkpoint is not bound to its confirmed settlement');
  }
  validatePriorSettlementState(prior);
  const entries = settlementDistributionEntries(prior, 'prior checkpoint settlement');
  if (entries.length === 0 || distribution(entries).root.toLowerCase() !== checkpoint.root) {
    throw new Error('prior TAP checkpoint root does not commit its distribution');
  }
  const liabilities = canonicalTapLiabilityMap(canonicalLiabilities);
  let providerNetAu = 0n;
  let providerTapWei = 0n;
  for (const output of checkpoint.outputs ?? []) {
    if (!isObject(output) ||
        !isHexBytes(output.provider, 32) ||
        !isHexBytes(output.payout_revision, 32)) {
      throw new Error('prior TAP checkpoint provider output is invalid');
    }
    const target = normalizeAddress(output.to, 'prior TAP checkpoint target');
    const identity = tapLiabilityIdentity(
      output.provider,
      output.payout_revision,
      target
    );
    if (!liabilities.has(identity)) {
      throw new Error(`canonical TAP liability is missing for checkpoint replay ${identity}`);
    }
    const previous = parseBigIntString(
      output.prior_cumulative_claim_wei,
      'prior TAP checkpoint previous cumulative wei'
    );
    const paidWei = parseBigIntString(output.tap_wei, 'prior TAP checkpoint TAP wei');
    const next = parseBigIntString(
      output.cumulative_claim_wei,
      'prior TAP checkpoint new cumulative wei'
    );
    if (next !== previous + paidWei) {
      throw new Error('prior TAP checkpoint cumulative wei does not conserve');
    }
    providerNetAu += safeAu(output.paid_au, 'prior TAP checkpoint net AU');
    providerTapWei += paidWei;
  }
  if (checkpoint.provider_count !== (checkpoint.outputs ?? []).length ||
      checkpoint.provider_paid_au !== providerNetAu.toString() ||
      checkpoint.provider_tap_wei !== providerTapWei.toString()) {
    throw new Error('prior TAP checkpoint provider totals do not conserve');
  }
  return prior;
}

async function rootEventRecord(event, label) {
  let blockHash = event?.blockHash;
  const isHash = (value) => (
    typeof value === 'string' &&
    /^0x[0-9a-f]{64}$/i.test(value)
  );
  if (!isHash(blockHash) && typeof event?.getBlock === 'function') {
    blockHash = (await event.getBlock())?.hash;
  }
  if (!event ||
      !isHash(event.transactionHash) ||
      !isHash(blockHash) ||
      !Number.isSafeInteger(event.blockNumber)) {
    throw new Error(`confirmed TAP ${label} event evidence is missing`);
  }
  const args = event.args;
  const transactionReceipt = typeof event.getTransactionReceipt === 'function'
    ? await event.getTransactionReceipt()
    : null;
  return {
    tx_hash: String(event.transactionHash).toLowerCase(),
    block_number: event.blockNumber,
    block_hash: String(blockHash).toLowerCase(),
    nonce: parseBigIntString(args?.nonce, `${label} nonce`).toString(),
    status: transactionReceipt === null ? null : Number(transactionReceipt.status),
  };
}

async function confirmedRootEventEvidence({
  pool,
  epoch,
  root,
  cumulativeSpentWei,
  priorCheckpoint,
  epochApplyHash,
  tapRateLock,
} = {}) {
  if (!pool?.queryFilter || !pool?.filters?.RootPosted || !pool?.filters?.RootProposed) {
    throw new Error('TAP pool event queries are required for checkpoint confirmation');
  }
  const provider = pool?.runner?.provider;
  if (!provider?.getBlock || !provider?.getNetwork) {
    throw new Error('TAP pool provider cannot prove settlement finality');
  }
  let queryThroughBlock = 'latest';
  if (typeof provider.send === 'function') {
    const rawBlockNumber = await provider.send('eth_blockNumber', []);
    const blockNumber = Number(BigInt(rawBlockNumber));
    if (!Number.isSafeInteger(blockNumber) || blockNumber < 0) {
      throw new Error('TAP pool returned an invalid latest block number');
    }
    queryThroughBlock = blockNumber;
  }
  const postedEvents = await pool.queryFilter(
    pool.filters.RootPosted(epoch),
    0,
    queryThroughBlock
  );
  const posted = postedEvents.find((event) => (
    String(event.args?.merkleRoot ?? '').toLowerCase() === root.toLowerCase() &&
    parseBigIntString(
      event.args?.cumulativeSpent,
      'RootPosted cumulative spent'
    ) === cumulativeSpentWei
  ));
  if (!posted) throw new Error('confirmed TAP RootPosted event is missing');
  const nonce = parseBigIntString(posted.args?.nonce, 'RootPosted nonce');
  const proposedEvents = await pool.queryFilter(
    pool.filters.RootProposed(epoch),
    0,
    queryThroughBlock
  );
  const proposed = proposedEvents.find((event) => (
    String(event.args?.merkleRoot ?? '').toLowerCase() === root.toLowerCase() &&
    parseBigIntString(
      event.args?.cumulativeSpent,
      'RootProposed cumulative spent'
    ) === cumulativeSpentWei &&
    parseBigIntString(event.args?.nonce, 'RootProposed nonce') === nonce
  ));
  if (!proposed) throw new Error('confirmed TAP RootProposed event is missing');
  const proposalRecord = await rootEventRecord(proposed, 'RootProposed');
  const executionRecord = await rootEventRecord(posted, 'RootPosted');
  if (executionRecord.status !== 1) {
    throw new Error('confirmed TAP RootPosted transaction did not succeed');
  }
  const network = await provider.getNetwork();
  let finalizedBlock;
  let confirmationPolicy;
  if (network.chainId === 1n) {
    finalizedBlock = await provider.getBlock('finalized');
    confirmationPolicy = 'finalized-tag';
  } else {
    finalizedBlock = await provider.getBlock('latest');
    confirmationPolicy = finalizedBlock
      ? `depth-${Number(finalizedBlock.number) - executionRecord.block_number}`
      : null;
  }
  const finalizedBlockNumber = Number(finalizedBlock?.number);
  const confirmationDepth = finalizedBlockNumber - executionRecord.block_number;
  if (!Number.isSafeInteger(finalizedBlockNumber) ||
      confirmationDepth < MIN_TAP_CONFIRMATION_DEPTH) {
    const error = new Error(
      `TAP root awaits ${MIN_TAP_CONFIRMATION_DEPTH} confirmations`
    );
    error.code = 'TAP_FINALITY_PENDING';
    error.finalizedBlockNumber = Number.isSafeInteger(finalizedBlockNumber)
      ? finalizedBlockNumber
      : null;
    error.confirmationDepth = Number.isSafeInteger(confirmationDepth)
      ? confirmationDepth
      : 0;
    throw error;
  }
  const evidence = {
    confirmed: true,
    onchain_epoch: epoch,
    onchain_root: root.toLowerCase(),
    onchain_cumulative_spent_wei: cumulativeSpentWei.toString(),
    proposal: {
      ...proposalRecord,
      execute_after: parseNonNegativeInt(
        proposed.args?.executeAfter,
        'RootProposed execute_after'
      ),
    },
    execution: executionRecord,
    finalized_block_number: finalizedBlockNumber,
    confirmation_depth: confirmationDepth,
    confirmation_policy: confirmationPolicy,
  };
  return evidence;
}

function tapCheckpointPayload({
  settlement,
  epoch,
  epochApplyHash,
  tapRateLock,
  rootConfirmation,
  preparationPlan,
} = {}) {
  if (!isHexBytes(epochApplyHash, 32)) {
    throw new Error('canonical TAP epoch_apply_hash is required for checkpoint evidence');
  }
  if (!isObject(tapRateLock)) {
    throw new Error('canonical TAP rate lock is required for checkpoint evidence');
  }
  if (!isHexBytes(tapRateLock.bundle_sha256, 32) ||
      typeof tapRateLock.rate_record_key !== 'string' ||
      tapRateLock.rate_record_key.length === 0 ||
      typeof tapRateLock.source !== 'string' ||
      tapRateLock.source.length === 0) {
    throw new Error('canonical TAP rate-lock identity is incomplete');
  }
  if (!isObject(preparationPlan?.intent)) {
    throw new Error('canonical TAP preparation plan is required for checkpoint evidence');
  }
  const intent = preparationPlan.intent;
  return {
    op: 'settle_targeted_tap',
    epoch,
    at: intent.at,
    rail: 'tap',
    chain_id: intent.chain_id,
    token_address: intent.token_address,
    pool_address: intent.pool_address,
    payment_config_ver: intent.payment_config_ver,
    epoch_apply_hash: epochApplyHash.toLowerCase(),
    preparation_ids: preparationPlan.preparation_ids,
    root_preparation_id: preparationPlan.root_preparation_id,
    external_effect_ids: preparationPlan.external_effect_ids,
    tap_rate_lock: tapRateLock,
    root: intent.root,
    root_confirmed: true,
    proposal_tx: rootConfirmation.proposal.tx_hash,
    proposal_block_number: rootConfirmation.proposal.block_number,
    proposal_block_hash: rootConfirmation.proposal.block_hash,
    execution_tx: rootConfirmation.execution.tx_hash,
    execution_status: rootConfirmation.execution.status,
    execution_block_number: rootConfirmation.execution.block_number,
    execution_block_hash: rootConfirmation.execution.block_hash,
    finalized_block_number: rootConfirmation.finalized_block_number,
    confirmation_depth: rootConfirmation.confirmation_depth,
    confirmation_policy: rootConfirmation.confirmation_policy,
    cumulative_spent_wei: intent.cumulative_spent_wei,
    provider_cumulative_claimed_wei: intent.provider_cumulative_claimed_wei,
    buyer_refund_wei: intent.buyer_refund_wei,
    fee_bps: intent.fee_bps,
    tap_burn_bps: intent.tap_burn_bps,
    provider_share_bps: intent.provider_share_bps,
    provider_count: intent.provider_count,
    provider_paid_au: intent.provider_paid_au,
    provider_tap_wei: intent.provider_tap_wei,
    provider_entries: intent.provider_entries,
    refunds: intent.refunds,
    entries: intent.entries,
    outputs: intent.outputs,
  };
}

export async function rollTapSettlement({
  bundle,
  receipts,
  targetedSessionBindings,
  canonicalLiabilities,
  buyerAccounts,
  tapUsdAu,
  ledgerFeeBps,
  prior,
  settleThroughEpoch,
  challengeEpochs,
  holdbackEpochs,
  epochApplyHash,
  tapRateLock,
  pool,
  ownerSigner,
  governanceSigner,
  operatorAddress,
  epoch,
  canonicalPreparationSubmitter,
  post = true,
} = {}) {
  const inputBundle = receipts ? { receipts } : bundle;
  tapPayoutMinAu(inputBundle);
  tapLedgerFeeBps(inputBundle, ledgerFeeBps);
  if (!isObject(targetedSessionBindings)) {
    throw new Error('confirmed targeted TAP session bindings are required');
  }
  if (isObject(tapRateLock) &&
      safeAu(tapUsdAu, 'tap_usd_au').toString() !== tapRateLock.tap_usd_au) {
    throw new Error('TAP settlement rate does not match its canonical rate lock');
  }
  const requestedEpoch = parseNonNegativeInt(
    epoch ?? inputBundle?.epoch ?? inputBundle?.settlement_epoch,
    'TAP settlement epoch'
  );
  const replaySettlement = checkpointReplaySettlement({
    prior,
    epoch: requestedEpoch,
    epochApplyHash,
    tapRateLock,
    canonicalLiabilities,
  });
  const settlement = replaySettlement ?? buildTapSettlement({
      bundle,
      receipts,
      buyerAccounts,
      tapUsdAu,
      ledgerFeeBps,
      prior,
      settleThroughEpoch,
      challengeEpochs,
      holdbackEpochs,
      targetedSessionBindings,
      canonicalLiabilities,
    });
  if (!settlement.root) return settlement;

  const expectedRoot = String(settlement.root).toLowerCase();
  const expectedSpentWei = parseBigIntString(
    settlement.cumulative_spent_wei,
    'settlement cumulative spent wei'
  );
  let chainEpoch = null;
  let chainSpentWei = null;
  let chainRoot = null;
  let pendingRoot = null;
  let rootAlreadyPosted = false;
  if (pool) {
    [chainEpoch, chainSpentWei, chainRoot, pendingRoot] = await Promise.all([
      pool.epoch().then(Number),
      pool.cumulativeSpent().then((value) => parseBigIntString(value, 'pool cumulative spent wei')),
      poolRootOrNull(pool),
      pool.pendingRoot().then(pendingRootRecord),
    ]);
    rootAlreadyPosted = (
      chainEpoch > 0
      && chainSpentWei === expectedSpentWei
      && chainRoot === expectedRoot
      && (epoch === undefined || parseNonNegativeInt(epoch, 'root epoch') === chainEpoch)
    );
  }

  let screen;
  if (rootAlreadyPosted) {
    const checked = await guardianPreSignReport({
      settlement,
      pool,
      epoch: chainEpoch,
      previous: prior,
      currentEpoch: chainEpoch - 1,
      prevSpentWei: chainSpentWei,
    });
    const reasons = checked.reasons.filter((reason) => reason !== 'no new spend since last root');
    screen = {
      ...checked,
      ok: reasons.length === 0,
      reasons,
      flags: [...checked.flags, 'exact settlement root already confirmed on-chain'],
      epoch: chainEpoch,
      resumed: true,
    };
  } else {
    screen = await guardianScreenSettlement({ settlement, pool, epoch, previous: prior });
  }
  if (!screen.ok) {
    return { ...settlement, posted: false, blocked: true, reasons: screen.reasons, screen };
  }

  if (pendingRoot && (
    pendingRoot.merkle_root !== expectedRoot
    || pendingRoot.epoch !== screen.epoch
    || pendingRoot.cumulative_spent_wei !== expectedSpentWei.toString()
  )) {
    return {
      ...settlement,
      posted: false,
      blocked: true,
      reasons: ['a different cross-signed TAP root is pending execution'],
      epoch: screen.epoch,
      pending_root: pendingRoot,
      screen,
    };
  }

  const preparationPlan = await buildCanonicalTapPreparationPlan({
    settlement,
    bundle: inputBundle,
    epoch: screen.epoch,
    epochApplyHash,
    tapRateLock,
  });
  const signingAddress = ownerSigner?.getAddress
    ? normalizeAddress(await ownerSigner.getAddress(), 'owner signing address')
    : null;
  const governanceSigningAddress = governanceSigner?.getAddress
    ? normalizeAddress(await governanceSigner.getAddress(), 'governance signing address')
    : null;
  const writable = pool && ownerSigner
    ? (pool.connect ? pool.connect(ownerSigner) : pool)
    : null;
  let governanceSignature = null;
  let proposeRootCalldata = null;
  let proposeRootDryRun = null;
  let executeRootDryRun = null;

  if (pool && !rootAlreadyPosted && !pendingRoot) {
    if (!ownerSigner) throw new Error('Missing TAP settlement owner signer');
    if (!governanceSigner) throw new Error('Missing independent TAP governance signer');
    governanceSignature = await signRootProposal({
      signer: governanceSigner,
      pool,
      merkleRoot: settlement.root,
      newEpoch: screen.epoch,
      newCumulativeSpent: expectedSpentWei,
    });
    proposeRootCalldata = encodeProposeRootCalldata({
      root: settlement.root,
      epoch: screen.epoch,
      cumulativeSpentWei: expectedSpentWei,
      governanceSignature,
    });
    proposeRootDryRun = await dryRunProposeRoot({
      writablePool: writable,
      root: settlement.root,
      epoch: screen.epoch,
      cumulativeSpentWei: expectedSpentWei,
      governanceSignature,
    });
    if (!proposeRootDryRun.ok) {
      return {
        ...settlement,
        posted: false,
        blocked: true,
        reasons: [`proposeRoot dry-run failed: ${proposeRootDryRun.error}`],
        epoch: screen.epoch,
        signing_address: signingAddress,
        governance_signing_address: governanceSigningAddress,
        propose_root_calldata: proposeRootCalldata,
        propose_root_dry_run: proposeRootDryRun,
        screen,
      };
    }
  } else if (rootAlreadyPosted) {
    proposeRootDryRun = {
      ok: true,
      skipped: true,
      reason: 'exact settlement root already confirmed on-chain',
    };
  } else if (pendingRoot) {
    proposeRootDryRun = {
      ok: true,
      skipped: true,
      reason: 'exact cross-signed root already pending',
    };
    if (await poolTimestamp(pool) >= pendingRoot.execute_after) {
      executeRootDryRun = await dryRunExecuteRoot({ writablePool: writable ?? pool });
    } else {
      executeRootDryRun = {
        ok: false,
        awaiting_governance_delay: true,
        execute_after: pendingRoot.execute_after,
      };
    }
  }

  let operatorFee = null;
  let burn = null;
  if (pool) {
    const operatorWithdrawn = await pool.operatorWithdrawn();
    const operatorCapWei = bpsOf(expectedSpentWei, OPERATOR_BPS);
    const predictedClaimableWei = operatorCapWei > operatorWithdrawn
      ? operatorCapWei - operatorWithdrawn
      : 0n;
    const providerCapWei = bpsOf(expectedSpentWei, PROVIDER_BPS);
    const burnCapWei = expectedSpentWei - providerCapWei - operatorCapWei;
    const totalBurned = await pool.totalBurned();
    const predictedBurnWei = burnCapWei > totalBurned ? burnCapWei - totalBurned : 0n;
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
      completed: false,
    };
    burn = {
      auto_sent: false,
      predicted_claimable_wei: predictedBurnWei.toString(),
      calldata: predictedBurnWei > 0n ? encodeBurnCalldata() : null,
      dry_run: null,
      tx: null,
      completed: false,
    };
    if (predictedClaimableWei > 0n && !destination) {
      return {
        ...settlement,
        posted: false,
        blocked: true,
        reasons: ['operator fee destination is required for TAP auto-withdraw'],
        epoch: screen.epoch,
        signing_address: signingAddress,
        governance_signing_address: governanceSigningAddress,
        propose_root_calldata: proposeRootCalldata,
        propose_root_dry_run: proposeRootDryRun,
        pending_root: pendingRoot,
        operator_fee: operatorFee,
        burn,
        screen,
      };
    }
  }

  let canonicalPreparation = null;
  if (post && pool) {
    if (typeof canonicalPreparationSubmitter !== 'function') {
      throw new Error(
        'TAP settlement broadcast requires canonical payout preparation submission'
      );
    }
    canonicalPreparation = await canonicalPreparationSubmitter({
      plan: preparationPlan,
    });
    if (!isObject(canonicalPreparation) ||
        stableJson(canonicalPreparation.preparation_ids) !==
          stableJson(preparationPlan.preparation_ids) ||
        canonicalPreparation.root_preparation_id !==
          preparationPlan.root_preparation_id ||
        stableJson(canonicalPreparation.external_effect_ids) !==
          stableJson(preparationPlan.external_effect_ids) ||
        !Array.isArray(canonicalPreparation.records) ||
        canonicalPreparation.records.length !== preparationPlan.preparations.length ||
        canonicalPreparation.records.some((record, index) => (
          record?.type !== 'targeted_payout_preparation' ||
          record?.economic_op_id !== preparationPlan.preparations[index].economic_op_id ||
          record?.consumed !== false
        ))) {
      throw new Error('canonical TAP preparation confirmation is incomplete or mismatched');
    }
    if ((rootAlreadyPosted || pendingRoot) &&
        canonicalPreparation.all_existing !== true) {
      throw new Error(
        'existing on-chain TAP root is not preceded by its canonical preparation'
      );
    }
  }
  const preparationFields = {
    preparation_ids: preparationPlan.preparation_ids,
    root_preparation_id: preparationPlan.root_preparation_id,
    external_effect_ids: preparationPlan.external_effect_ids,
    canonical_preparation_confirmed: canonicalPreparation !== null,
    epoch_apply_hash: preparationPlan.intent.epoch_apply_hash,
    tap_rate_lock: preparationPlan.intent.tap_rate_lock,
  };

  let proposalTx = null;
  let executionTx = null;
  let proposalBlockNumber = null;
  let rootConfirmed = rootAlreadyPosted;
  if (post && pool) {
    if (!ownerSigner) throw new Error('Missing TAP settlement owner signer for broadcast');
    if (!writable) throw new Error('Missing writable TAP pool');
    if (!rootAlreadyPosted && !pendingRoot) {
      const sent = await writable.proposeRoot(
        settlement.root,
        screen.epoch,
        expectedSpentWei,
        governanceSignature
      );
      const receipt = await sent.wait();
      proposalTx = sent.hash;
      proposalBlockNumber = receipt?.blockNumber ?? null;
      pendingRoot = pendingRootRecord(await pool.pendingRoot());
      if (!pendingRoot
        || pendingRoot.merkle_root !== expectedRoot
        || pendingRoot.epoch !== screen.epoch
        || pendingRoot.cumulative_spent_wei !== expectedSpentWei.toString()) {
        return {
          ...settlement,
          ...preparationFields,
          posted: false,
          blocked: true,
          reasons: ['proposeRoot transaction did not create the exact expected pending root'],
          epoch: screen.epoch,
          proposal_tx: proposalTx,
          signing_address: signingAddress,
          governance_signing_address: governanceSigningAddress,
          propose_root_calldata: proposeRootCalldata,
          propose_root_dry_run: proposeRootDryRun,
          pending_root: pendingRoot,
          operator_fee: operatorFee,
          burn,
          screen,
        };
      }
    }

    if (!rootAlreadyPosted && pendingRoot) {
      const now = await poolTimestamp(pool, proposalBlockNumber ?? 'latest');
      if (now < pendingRoot.execute_after) {
        return {
          ...settlement,
          ...preparationFields,
          posted: false,
          root_proposed: true,
          root_pending: true,
          root_confirmed: false,
          awaiting_governance_delay: true,
          execute_after: pendingRoot.execute_after,
          epoch: screen.epoch,
          proposal_tx: proposalTx,
          signing_address: signingAddress,
          governance_signing_address: governanceSigningAddress,
          propose_root_calldata: proposeRootCalldata,
          propose_root_dry_run: proposeRootDryRun,
          execute_root_calldata: encodeExecuteRootCalldata(),
          execute_root_dry_run: executeRootDryRun,
          pending_root: pendingRoot,
          operator_fee: operatorFee,
          burn,
          screen,
        };
      }
      executeRootDryRun = await dryRunExecuteRoot({ writablePool: writable });
      if (!executeRootDryRun.ok) {
        return {
          ...settlement,
          ...preparationFields,
          posted: false,
          blocked: true,
          reasons: [`executeRoot dry-run failed: ${executeRootDryRun.error}`],
          epoch: screen.epoch,
          proposal_tx: proposalTx,
          execute_root_calldata: encodeExecuteRootCalldata(),
          execute_root_dry_run: executeRootDryRun,
          pending_root: pendingRoot,
          operator_fee: operatorFee,
          burn,
          screen,
        };
      }
      const sent = await writable.executeRoot();
      await sent.wait();
      executionTx = sent.hash;
      const [confirmedEpoch, confirmedSpentWei, confirmedRoot] = await Promise.all([
        pool.epoch().then(Number),
        pool.cumulativeSpent().then((value) => parseBigIntString(value, 'confirmed cumulative spent wei')),
        poolRootOrNull(pool),
      ]);
      rootConfirmed = (
        confirmedEpoch === screen.epoch
        && confirmedSpentWei === expectedSpentWei
        && confirmedRoot === expectedRoot
      );
      if (!rootConfirmed) {
        return {
          ...settlement,
          ...preparationFields,
          posted: false,
          root_proposed: true,
          root_confirmed: false,
          blocked: true,
          reasons: ['executeRoot transaction did not produce the exact expected on-chain state'],
          epoch: screen.epoch,
          proposal_tx: proposalTx,
          execution_tx: executionTx,
          operator_fee: operatorFee,
          burn,
          screen,
        };
      }
    }

    if (rootConfirmed && operatorFee) {
      const claimable = await pool.operatorClaimable();
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
            ...preparationFields,
            posted: rootConfirmed,
            root_confirmed: rootConfirmed,
            blocked: true,
            reasons: [`withdrawOperator dry-run failed: ${operatorFee.dry_run.error}`],
            epoch: screen.epoch,
            proposal_tx: proposalTx,
            execution_tx: executionTx,
            operator_fee: operatorFee,
            burn,
            screen,
          };
        }
        const feeSent = await writable.withdrawOperator(operatorFee.destination, claimable);
        await feeSent.wait();
        operatorFee.tx = feeSent.hash;
        operatorFee.auto_sent = true;
      }
      const remainingFee = await pool.operatorClaimable();
      operatorFee.remaining_claimable_wei = remainingFee.toString();
      operatorFee.completed = remainingFee === 0n;
      if (!operatorFee.completed) {
        return {
          ...settlement,
          ...preparationFields,
          posted: rootConfirmed,
          root_confirmed: rootConfirmed,
          blocked: true,
          reasons: ['operator fee remained claimable after auto-withdraw'],
          epoch: screen.epoch,
          proposal_tx: proposalTx,
          execution_tx: executionTx,
          operator_fee: operatorFee,
          burn,
          screen,
        };
      }
    }

    if (rootConfirmed && burn) {
      const claimable = await pool.burnClaimable();
      burn.actual_claimable_wei = claimable.toString();
      if (claimable > 0n) {
        burn.dry_run = await dryRunBurn({ writablePool: writable });
        if (!burn.dry_run.ok) {
          return {
            ...settlement,
            ...preparationFields,
            posted: rootConfirmed,
            root_confirmed: rootConfirmed,
            blocked: true,
            reasons: [`burn dry-run failed: ${burn.dry_run.error}`],
            epoch: screen.epoch,
            proposal_tx: proposalTx,
            execution_tx: executionTx,
            operator_fee: operatorFee,
            burn,
            screen,
          };
        }
        const burnSent = await writable.burn();
        await burnSent.wait();
        burn.tx = burnSent.hash;
        burn.auto_sent = true;
      }
      const remainingBurn = await pool.burnClaimable();
      burn.remaining_claimable_wei = remainingBurn.toString();
      burn.total_burned_wei = (await pool.totalBurned()).toString();
      burn.completed = remainingBurn === 0n;
      if (!burn.completed) {
        return {
          ...settlement,
          ...preparationFields,
          posted: rootConfirmed,
          root_confirmed: rootConfirmed,
          blocked: true,
          reasons: ['TAP burn remained claimable after automatic burn'],
          epoch: screen.epoch,
          proposal_tx: proposalTx,
          execution_tx: executionTx,
          operator_fee: operatorFee,
          burn,
          screen,
        };
      }
    }
  }

  let tapSettlementCheckpoint = null;
  if (rootConfirmed) {
    let rootConfirmation;
    try {
      rootConfirmation = await confirmedRootEventEvidence({
        pool,
        epoch: screen.epoch,
        root: expectedRoot,
        cumulativeSpentWei: expectedSpentWei,
        priorCheckpoint: prior?.tap_settlement_checkpoint,
        epochApplyHash,
        tapRateLock,
      });
    } catch (error) {
      if (error?.code !== 'TAP_FINALITY_PENDING') throw error;
      return {
        ...settlement,
        ...preparationFields,
        posted: rootConfirmed && !rootAlreadyPosted,
        root_proposed: true,
        root_confirmed: true,
        root_already_posted: rootAlreadyPosted,
        root_pending: false,
        awaiting_finality: true,
        confirmation_depth: error.confirmationDepth,
        finalized_block_number: error.finalizedBlockNumber,
        epoch: screen.epoch,
        proposal_tx: proposalTx,
        execution_tx: executionTx,
        signing_address: signingAddress,
        governance_signing_address: governanceSigningAddress,
        operator_fee: operatorFee,
        burn,
        tap_settlement_checkpoint: null,
        screen,
      };
    }
    tapSettlementCheckpoint = tapCheckpointPayload({
      settlement,
      epoch: screen.epoch,
      epochApplyHash,
      tapRateLock,
      rootConfirmation,
      preparationPlan,
    });
  }

  return {
    ...settlement,
    ...preparationFields,
    posted: rootConfirmed && !rootAlreadyPosted,
    root_proposed: Boolean(proposalTx || pendingRoot),
    root_confirmed: rootConfirmed,
    root_already_posted: rootAlreadyPosted,
    root_pending: Boolean(pendingRoot && !rootConfirmed),
    epoch: screen.epoch,
    proposal_tx: proposalTx,
    execution_tx: executionTx,
    signing_address: signingAddress,
    governance_signing_address: governanceSigningAddress,
    propose_root_calldata: proposeRootCalldata,
    propose_root_dry_run: proposeRootDryRun,
    execute_root_calldata: pendingRoot && !rootConfirmed ? encodeExecuteRootCalldata() : null,
    execute_root_dry_run: executeRootDryRun,
    pending_root: pendingRoot && !rootConfirmed ? pendingRoot : null,
    operator_fee: operatorFee,
    burn,
    tap_settlement_checkpoint: tapSettlementCheckpoint,
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

export async function resolveTargetedTapPayoutsFromLedger({
  bundle,
  peerRpcUrl,
  fetchImpl,
} = {}) {
  const bundleEpoch = bundle?.epoch ?? bundle?.receipt_epoch ?? bundle?.settlement_epoch;
  const sessionBindings = {};
  const accounts = {};
  const liabilities = new Map();
  const aggregatePaidByProvider = new Map();
  for (const entry of normalizedReceipts(bundle)) {
    const { body, envelope } = receiptEnvelope(entry);
    verifyReceiptEnvelope(envelope);
    const epoch = parsePositiveInt(
      entryEpoch(entry, body, bundleEpoch),
      'targeted TAP receipt epoch'
    );
    const key = targetedTapSessionBindingKey({
      epoch,
      user: body.user,
      sessionId: body.session_id,
    });
    const allocation = await readContractStateValue(
      peerRpcUrl,
      `payout/allocation/${epoch}/${body.session_id}`,
      { confirmed: true, fetchImpl }
    );
    if (!allocation ||
        allocation.type !== 'provider_payout_session_allocation' ||
        allocation.epoch !== epoch ||
        allocation.session_id !== body.session_id ||
        allocation.user !== body.user ||
        allocation.rail !== 'tap' ||
        allocation.provider !== body.provider ||
        !isHexBytes(allocation.payout_revision, 32) ||
        !String(allocation.feature_key ?? '').startsWith(`epoch/targeted/${epoch}/`)) {
      throw new Error(`confirmed targeted TAP allocation is missing for session ${body.session_id}`);
    }
    const allocationAu = safeAu(allocation.au, 'targeted TAP allocation au').toString();
    const revision = allocation.payout_revision.toLowerCase();
    const binding = await readContractStateValue(
      peerRpcUrl,
      `payout/binding/tap/${body.provider}/${revision}`,
      { confirmed: true, fetchImpl }
    );
    if (!binding ||
        binding.verified !== true ||
        binding.provider !== body.provider ||
        binding.rail !== 'tap' ||
        binding.revision !== revision ||
        binding.activation_epoch > epoch ||
        !Number.isSafeInteger(binding.chain_id) ||
        binding.chain_id < 1) {
      throw new Error(`verified targeted TAP binding is missing for session ${body.session_id}`);
    }
    const liability = await readContractStateValue(
      peerRpcUrl,
      `payout/liability/tap/${body.provider}/${revision}`,
      { confirmed: true, fetchImpl }
    );
    if (!liability ||
        liability.provider !== body.provider ||
        liability.rail !== 'tap' ||
        liability.revision !== revision ||
        liability.target !== binding.target ||
        liability.chain_id !== binding.chain_id) {
      throw new Error(`targeted TAP liability is missing for session ${body.session_id}`);
    }
    let aggregatePaidCumAu = aggregatePaidByProvider.get(body.provider);
    if (aggregatePaidCumAu === undefined) {
      const earning = await readContractStateValue(
        peerRpcUrl,
        `earn/tap/${body.provider}`,
        { confirmed: true, fetchImpl }
      );
      if (!earning ||
          earning.provider !== body.provider ||
          earning.rail !== 'tap') {
        throw new Error(
          `targeted TAP aggregate earning watermark is missing for provider ${body.provider}`
        );
      }
      aggregatePaidCumAu = safeAu(
        earning.paid_cum_au,
        'targeted TAP aggregate earning paid_cum_au',
        { allowZero: true }
      ).toString();
      aggregatePaidByProvider.set(body.provider, aggregatePaidCumAu);
    }
    const liabilityTarget = normalizeAddress(
      liability.target,
      `targeted TAP liability target for ${body.provider}`
    );
    const normalizedLiability = {
      provider: body.provider,
      rail: 'tap',
      payout_revision: revision,
      target: liabilityTarget,
      chain_id: liability.chain_id,
      total_au: safeAu(
        liability.total_au,
        'targeted TAP liability total_au',
        { allowZero: true }
      ).toString(),
      held_au: safeAu(
        liability.held_au,
        'targeted TAP liability held_au',
        { allowZero: true }
      ).toString(),
      paid_cum_au: safeAu(
        liability.paid_cum_au,
        'targeted TAP liability paid_cum_au',
        { allowZero: true }
      ).toString(),
      aggregate_paid_cum_au: aggregatePaidCumAu,
      updated_epoch: parseNonNegativeInt(
        liability.updated_epoch,
        'targeted TAP liability updated_epoch',
        0
      ),
      updated_at: liability.updated_at ?? null,
    };
    const liabilityKey = tapLiabilityIdentity(body.provider, revision, liabilityTarget);
    const existingLiability = liabilities.get(liabilityKey);
    if (existingLiability &&
        stableJson(existingLiability) !== stableJson(normalizedLiability)) {
      throw new Error(`conflicting targeted TAP liability for provider ${body.provider}`);
    }
    liabilities.set(liabilityKey, normalizedLiability);
    const resolved = {
      epoch,
      session_id: body.session_id,
      user: body.user,
      provider: body.provider,
      payout_revision: revision,
      account: normalizeAddress(
        binding.target,
        `targeted TAP account for ${body.provider}`
      ),
      chain_id: binding.chain_id,
      context_revision: binding.context_revision,
      payment_config_version: binding.payment_config_version,
      au: allocationAu,
    };
    if (sessionBindings[key] &&
        stableJson(sessionBindings[key]) !== stableJson(resolved)) {
      throw new Error(`conflicting targeted TAP allocation for session ${body.session_id}`);
    }
    sessionBindings[key] = resolved;
    const providerRevision = `${body.provider}/${revision}`;
    accounts[providerRevision] = resolved.account;
  }
  return {
    accounts,
    sessionBindings,
    liabilities: sortedTapLiabilities(canonicalTapLiabilityMap([...liabilities.values()]))
      .map((entry) => ({
        provider: entry.provider,
        rail: 'tap',
        payout_revision: entry.payout_revision,
        target: entry.target,
        chain_id: entry.chain_id,
        total_au: entry.total_au.toString(),
        held_au: entry.held_au.toString(),
        paid_cum_au: entry.paid_cum_au.toString(),
        aggregate_paid_cum_au: entry.aggregate_paid_cum_au.toString(),
        updated_epoch: entry.updated_epoch,
        updated_at: entry.updated_at,
      })),
  };
}

async function resolveActiveEpochParam({ peerRpcUrl, key, fallback, fetchImpl } = {}) {
  const record = await readContractStateValue(peerRpcUrl, `params/${key}`, { fetchImpl });
  const now = Math.floor(Date.now() / 1_000);
  const active = record?.pending && Number(record.pending.effective_at) <= now
    ? record.pending
    : record?.current;
  return parseNonNegativeInt(active?.value ?? fallback, `active ${key}`);
}

export async function resolveTapSettlementEpochPolicy({
  peerRpcUrl,
  fetchImpl,
} = {}) {
  const applyState = await readContractStateValue(peerRpcUrl, 'epoch/apply/state', { fetchImpl });
  const resolvedThrough = parseOptionalNonNegativeInt(
    applyState?.updated_epoch,
    'settle through epoch'
  );
  if (resolvedThrough === null) {
    throw new Error('epoch/apply/state.updated_epoch is required for TAP settlement');
  }
  const resolvedChallenge = await resolveActiveEpochParam({
    peerRpcUrl,
    key: 'challenge_epochs',
    fallback: DEFAULT_TAP_CHALLENGE_EPOCHS,
    fetchImpl,
  });
  return {
    settleThroughEpoch: resolvedThrough,
    challengeEpochs: parseNonNegativeInt(resolvedChallenge, 'TAP settlement challenge_epochs'),
  };
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (isObject(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(',')}}`;
  }
  return JSON.stringify(value);
}

async function preparationDependencies() {
  preparationDependenciesPromise ??= Promise.all([
    import('../../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs'),
    import('../../intercom/node_modules/b4a/index.js'),
    import('../../intercom/node_modules/trac-wallet/index.js'),
  ]).then(([blake3Module, b4aModule, walletModule]) => ({
    blake3: blake3Module.blake3,
    b4a: b4aModule.default,
    PeerWallet: walletModule.default,
  }));
  return preparationDependenciesPromise;
}

async function opaqueBlake3Hash(domain, value) {
  const { blake3, b4a } = await preparationDependencies();
  return b4a.toString(
    await blake3(b4a.from(stableJson({ domain, value }))),
    'hex'
  );
}

function tapPreparationOutput(output) {
  return {
    provider: output.provider,
    payout_revision: output.payout_revision,
    to: normalizeAddress(output.to, 'TAP preparation payout target'),
    paid_cum_au_before: safeAu(
      output.paid_cum_au_before,
      'TAP preparation liability watermark',
      { allowZero: true }
    ).toString(),
    aggregate_paid_cum_au_before: safeAu(
      output.aggregate_paid_cum_au_before,
      'TAP preparation aggregate watermark',
      { allowZero: true }
    ).toString(),
    paid_au: safeAu(output.net_au_paid, 'TAP preparation paid amount').toString(),
    tap_wei: parseBigIntString(output.tap_wei, 'TAP preparation tap_wei').toString(),
    prior_cumulative_claim_wei: parseBigIntString(
      output.previous_cumulative_wei,
      'TAP preparation prior cumulative claim'
    ).toString(),
    cumulative_claim_wei: parseBigIntString(
      output.new_cumulative_wei,
      'TAP preparation cumulative claim'
    ).toString(),
  };
}

function tapPreparationAt(bundle, tapRateLock) {
  return parseNonNegativeInt(
    bundle?.at
      ?? bundle?.settlement_unix
      ?? bundle?.last_settlement_unix
      ?? tapRateLock?.rate_ts,
    'TAP preparation timestamp'
  );
}

function tapPreparationOutputPayload(intent, output, outputIndex) {
  return {
    settlement_op: intent.op,
    rail: 'tap',
    epoch: intent.epoch,
    epoch_apply_hash: intent.epoch_apply_hash,
    output_index: outputIndex,
    output,
    chain_id: intent.chain_id,
    token_address: intent.token_address,
    pool_address: intent.pool_address,
    payment_config_ver: intent.payment_config_ver,
    fee_bps: intent.fee_bps,
    tap_burn_bps: intent.tap_burn_bps,
    provider_share_bps: intent.provider_share_bps,
    tap_rate_lock: intent.tap_rate_lock,
  };
}

function tapPreparationLiability(intent, output) {
  return {
    provider: output.provider,
    payout_revision: output.payout_revision,
    target: output.to,
    currency: null,
    chain_id: intent.chain_id,
    paid_cum_au_before: output.paid_cum_au_before,
    aggregate_paid_cum_au_before: output.aggregate_paid_cum_au_before,
    liability_au: output.paid_au,
    paid_au: output.paid_au,
  };
}

async function tapRootPreparationPayload(intent) {
  return {
    settlement_op: intent.op,
    rail: 'tap',
    epoch: intent.epoch,
    epoch_apply_hash: intent.epoch_apply_hash,
    chain_id: intent.chain_id,
    token_address: intent.token_address,
    pool_address: intent.pool_address,
    payment_config_ver: intent.payment_config_ver,
    tap_rate_lock: intent.tap_rate_lock,
    root: intent.root,
    cumulative_spent_wei: intent.cumulative_spent_wei,
    provider_cumulative_claimed_wei: intent.provider_cumulative_claimed_wei,
    buyer_refund_wei: intent.buyer_refund_wei,
    provider_count: intent.provider_count,
    provider_paid_au: intent.provider_paid_au,
    provider_tap_wei: intent.provider_tap_wei,
    fee_bps: intent.fee_bps,
    tap_burn_bps: intent.tap_burn_bps,
    provider_share_bps: intent.provider_share_bps,
    entries_hash: await opaqueBlake3Hash(
      'mayhem-targeted-tap-preparation-entries-v1',
      intent.entries
    ),
    provider_entries_hash: await opaqueBlake3Hash(
      'mayhem-targeted-tap-preparation-provider-entries-v1',
      intent.provider_entries
    ),
    refunds_hash: await opaqueBlake3Hash(
      'mayhem-targeted-tap-preparation-refunds-v1',
      intent.refunds
    ),
    outputs_hash: await opaqueBlake3Hash(
      'mayhem-targeted-tap-preparation-outputs-v1',
      intent.outputs
    ),
  };
}

export async function buildCanonicalTapPreparationPlan({
  settlement,
  bundle,
  epoch,
  epochApplyHash,
  tapRateLock,
} = {}) {
  if (!settlement?.root) throw new Error('TAP preparation requires a settlement root');
  const safeEpoch = parsePositiveInt(epoch, 'TAP preparation epoch');
  if (!isHexBytes(epochApplyHash, 32)) {
    throw new Error('TAP preparation requires a canonical epoch_apply_hash');
  }
  if (!isObject(tapRateLock)) {
    throw new Error('TAP preparation requires its canonical rate lock');
  }
  if (tapLedgerFeeBps(bundle, settlement.ledger_fee_bps) !== Number(OPERATOR_BPS)) {
    throw new Error('TAP preparation fee split is not canonical');
  }
  const outputs = (settlement.checkpoint_outputs ?? [])
    .map(tapPreparationOutput)
    .sort((left, right) => (
      left.provider.localeCompare(right.provider) ||
      left.payout_revision.localeCompare(right.payout_revision)
    ));
  if (outputs.length === 0) {
    throw new Error('TAP preparation requires at least one provider liability');
  }
  const providerPaidAu = outputs.reduce(
    (sum, output) => sum + BigInt(output.paid_au),
    0n
  );
  const providerTapWei = outputs.reduce(
    (sum, output) => sum + BigInt(output.tap_wei),
    0n
  );
  const intent = {
    op: 'settle_targeted_tap',
    epoch: safeEpoch,
    at: tapPreparationAt(bundle, tapRateLock),
    rail: 'tap',
    chain_id: parsePositiveInt(tapRateLock.chain_id, 'TAP preparation chain_id'),
    token_address: normalizeAddress(
      tapRateLock.token_address,
      'TAP preparation token address'
    ),
    pool_address: normalizeAddress(
      tapRateLock.pool_address,
      'TAP preparation pool address'
    ),
    payment_config_ver: parsePositiveInt(
      tapRateLock.payment_config_ver,
      'TAP preparation payment_config_ver'
    ),
    epoch_apply_hash: epochApplyHash.toLowerCase(),
    tap_rate_lock: tapRateLock,
    root: String(settlement.root).toLowerCase(),
    cumulative_spent_wei: parseBigIntString(
      settlement.cumulative_spent_wei,
      'TAP preparation cumulative gross spend'
    ).toString(),
    provider_cumulative_claimed_wei: parseBigIntString(
      settlement.provider_claimed_wei,
      'TAP preparation cumulative provider claims'
    ).toString(),
    buyer_refund_wei: parseBigIntString(
      settlement.buyer_refund_wei,
      'TAP preparation cumulative buyer refunds'
    ).toString(),
    fee_bps: Number(OPERATOR_BPS),
    tap_burn_bps: Number(TAP_BURN_BPS),
    provider_share_bps: Number(PROVIDER_BPS),
    provider_count: outputs.length,
    provider_paid_au: providerPaidAu.toString(),
    provider_tap_wei: providerTapWei.toString(),
    provider_entries: settlement.providers.map((entry) => ({
      account: normalizeAddress(entry.account, 'TAP preparation provider account'),
      cumulative_wei: parseBigIntString(
        entry.cumulative_wei,
        'TAP preparation provider cumulative claim'
      ).toString(),
    })),
    refunds: settlement.refunds.map((entry) => ({
      account: normalizeAddress(entry.account, 'TAP preparation refund account'),
      cumulative_wei: parseBigIntString(
        entry.cumulative_wei,
        'TAP preparation refund cumulative claim'
      ).toString(),
    })),
    entries: settlement.entries.map((entry) => ({
      account: normalizeAddress(entry.account, 'TAP preparation distribution account'),
      cumulative_wei: parseBigIntString(
        entry.cumulative_wei,
        'TAP preparation distribution cumulative claim'
      ).toString(),
    })),
    outputs,
  };
  const preparations = [];
  for (const [outputIndex, output] of outputs.entries()) {
    const payload = tapPreparationOutputPayload(intent, output, outputIndex);
    const liability = tapPreparationLiability(intent, output);
    const economicOpId = await opaqueBlake3Hash(
      'mayhem-targeted-tap-liability-preparation-id-v1',
      {
        epoch: intent.epoch,
        epoch_apply_hash: intent.epoch_apply_hash,
        output_index: outputIndex,
        payload,
        liability,
      }
    );
    preparations.push({
      economic_op_id: economicOpId,
      kind: 'liability',
      output_index: outputIndex,
      payload,
      liability,
      external_effect_ids: [],
    });
  }
  const externalEffectIds = await Promise.all(
    ['propose_root', 'execute_root'].map((action) => opaqueBlake3Hash(
      `mayhem-targeted-tap-${action}-effect-id-v1`,
      {
        chain_id: intent.chain_id,
        pool_address: intent.pool_address,
        epoch: intent.epoch,
        root: intent.root,
        cumulative_spent_wei: intent.cumulative_spent_wei,
      }
    ))
  );
  const rootPayload = await tapRootPreparationPayload(intent);
  const rootPreparationId = await opaqueBlake3Hash(
    'mayhem-targeted-tap-root-preparation-id-v1',
    rootPayload
  );
  preparations.push({
    economic_op_id: rootPreparationId,
    kind: 'tap_root',
    output_index: 0,
    payload: rootPayload,
    liability: null,
    external_effect_ids: externalEffectIds,
  });
  return {
    intent,
    preparation_ids: preparations
      .filter((entry) => entry.kind === 'liability')
      .map((entry) => entry.economic_op_id),
    root_preparation_id: rootPreparationId,
    external_effect_ids: externalEffectIds,
    preparations,
  };
}

function payoutPreparationEvidence(value) {
  return {
    op: value.op,
    contract_version: value.contract_version,
    economic_op_id: value.economic_op_id,
    rail: value.rail,
    epoch: value.epoch,
    epoch_apply_hash: value.epoch_apply_hash,
    prepared_at: value.prepared_at,
    kind: value.kind,
    output_index: value.output_index,
    payload_hash: value.payload_hash,
    payload: value.payload,
    liability: value.liability,
    external_effect_ids: value.external_effect_ids,
    admin: value.admin,
  };
}

function exactUnconsumedPreparation(record, value) {
  return isObject(record) &&
    record.type === 'targeted_payout_preparation' &&
    record.consumed === false &&
    Object.entries(value).every(([key, expected]) =>
      stableJson(record[key]) === stableJson(expected)
    );
}

async function submitPreparationFeature({
  peerRpcUrl,
  value,
  fetchImpl = globalThis.fetch,
  timeoutMs = PREPARATION_CONFIRM_TIMEOUT_MS,
  intervalMs = PREPARATION_CONFIRM_INTERVAL_MS,
} = {}) {
  const recordKey = `payout/preparation/tap/${value.economic_op_id}`;
  const existing = await readContractStateValue(
    peerRpcUrl,
    recordKey,
    { confirmed: true, fetchImpl }
  );
  if (existing !== null) {
    if (!exactUnconsumedPreparation(existing, value)) {
      throw new Error(
        `canonical TAP preparation ${value.economic_op_id} conflicts with the planned operation`
      );
    }
    return { record: existing, existing: true };
  }
  const featureDigest = await opaqueBlake3Hash(
    'mayhem-targeted-payout-preparation-feature-v1',
    value
  );
  const url = new URL('contract/feature', ensureRpcBase(peerRpcUrl));
  const response = await fetchImpl(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      feature: 'mayhem',
      key: (
        `payout/preparation-submit/tap/${value.economic_op_id}/` +
        featureDigest
      ),
      value,
    }),
  });
  if (!response?.ok) {
    throw new Error(
      `canonical TAP preparation submission failed with ${response?.status ?? 'unknown status'}`
    );
  }
  const submitted = await response.json();
  if (submitted?.ok !== true) {
    throw new Error(
      `canonical TAP preparation was not accepted: ${stableJson(submitted)}`
    );
  }
  const deadline = Date.now() + timeoutMs;
  while (Date.now() <= deadline) {
    const confirmed = await readContractStateValue(
      peerRpcUrl,
      recordKey,
      { confirmed: true, fetchImpl }
    );
    if (confirmed !== null) {
      if (!exactUnconsumedPreparation(confirmed, value)) {
        throw new Error(
          `confirmed TAP preparation ${value.economic_op_id} conflicts with the submitted operation`
        );
      }
      return { record: confirmed, existing: false };
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  throw new Error(
    `timed out confirming canonical TAP preparation ${value.economic_op_id}`
  );
}

export async function submitCanonicalTapPreparationPlan({
  plan,
  adminSigner,
  peerRpcUrl,
  fetchImpl,
  timeoutMs,
  intervalMs,
} = {}) {
  if (!isObject(plan) || !Array.isArray(plan.preparations)) {
    throw new Error('canonical TAP preparation plan is missing');
  }
  const admin = String(adminSigner?.publicKey ?? '').toLowerCase();
  if (!isHexBytes(admin, 32) || typeof adminSigner?.sign !== 'function') {
    throw new Error('canonical TAP preparation requires an Ed25519 admin signer');
  }
  const canonicalAdmin = await readContractStateValue(
    peerRpcUrl,
    'admin',
    { confirmed: true, fetchImpl }
  );
  if (String(canonicalAdmin ?? '').toLowerCase() !== admin) {
    throw new Error(`TAP preparation signer ${admin} is not the canonical admin`);
  }
  const results = [];
  for (const preparation of plan.preparations) {
    const payloadHash = await opaqueBlake3Hash(
      'mayhem-targeted-payout-preparation-payload-v1',
      {
        economic_op_id: preparation.economic_op_id,
        rail: 'tap',
        epoch: plan.intent.epoch,
        epoch_apply_hash: plan.intent.epoch_apply_hash,
        kind: preparation.kind,
        output_index: preparation.output_index,
        payload: preparation.payload,
      }
    );
    const unsigned = {
      op: 'prepare_targeted_payout',
      contract_version: CONTRACT_VERSION,
      economic_op_id: preparation.economic_op_id,
      rail: 'tap',
      epoch: plan.intent.epoch,
      epoch_apply_hash: plan.intent.epoch_apply_hash,
      prepared_at: plan.intent.at,
      kind: preparation.kind,
      output_index: preparation.output_index,
      payload_hash: payloadHash,
      payload: preparation.payload,
      liability: preparation.liability,
      external_effect_ids: preparation.external_effect_ids,
      admin,
    };
    const message = (
      `mayhem-targeted-payout-preparation-v1` +
      stableJson(payoutPreparationEvidence(unsigned))
    );
    const signature = String(await adminSigner.sign(message)).toLowerCase();
    if (!isHexBytes(signature, 64)) {
      throw new Error('canonical TAP preparation signer returned an invalid signature');
    }
    results.push(await submitPreparationFeature({
      peerRpcUrl,
      value: { ...unsigned, admin_sig: signature },
      fetchImpl,
      timeoutMs,
      intervalMs,
    }));
  }
  return {
    ...plan,
    all_existing: results.every((entry) => entry.existing),
    records: results.map((entry) => entry.record),
  };
}

export async function tapPreparationAdminSignerFromEnv(env = process.env) {
  const keypairPath = String(env.MAYHEM_TRAC_ADMIN_KEYPAIR_PATH ?? '').trim();
  if (!keypairPath) {
    throw new Error('Missing MAYHEM_TRAC_ADMIN_KEYPAIR_PATH for TAP preparation signing');
  }
  const stat = fs.lstatSync(keypairPath);
  if (!stat.isFile() || stat.isSymbolicLink() || (stat.mode & 0o077) !== 0) {
    throw new Error('TAP admin keypair must be a private regular file');
  }
  const { PeerWallet, b4a } = await preparationDependencies();
  const wallet = new PeerWallet();
  await wallet.ready;
  const password = String(
    env.MAYHEM_ADMIN_WALLET_PASSWORD
      ?? env.MAYHEM_WALLET_PASSWORD
      ?? ''
  );
  const originalLog = console.log;
  try {
    console.log = () => {};
    await wallet.importFromFile(keypairPath, b4a.from(password, 'utf8'));
  } finally {
    console.log = originalLog;
  }
  return {
    publicKey: b4a.toString(wallet.publicKey, 'hex'),
    sign(message) {
      return b4a.toString(wallet.sign(b4a.from(message, 'utf8')), 'hex');
    },
  };
}

function settlementBundleSha256(bundle) {
  return crypto.createHash('sha256').update(stableJson(bundle)).digest('hex');
}

function canonicalTapPaymentScope(payments, admin) {
  if (!isObject(payments) || payments.denom !== 'au_usd') {
    throw new Error('confirmed payments/current is missing or invalid');
  }
  if (!isHexBytes(admin, 32)
    || String(payments.set_by ?? '').toLowerCase() !== admin.toLowerCase()
    || payments.set_by_role !== 'admin') {
    throw new Error('payments/current was not posted by the current admin');
  }
  const chainId = parsePositiveInt(payments.tap?.chain_id, 'payments/current.tap.chain_id');
  const tokenAddress = normalizeAddress(
    payments.tap?.token_address,
    'payments/current.tap.token_address'
  );
  const poolAddress = normalizeAddress(
    payments.tap?.pool_address,
    'payments/current.tap.pool_address'
  );
  const paymentConfigVer = parsePositiveInt(payments.ver, 'payments/current.ver');
  return {
    chain_id: chainId,
    token_address: tokenAddress,
    pool_address: poolAddress,
    payment_config_ver: paymentConfigVer,
  };
}

function tapRateRecordToLock(rate, bundle, admin, paymentScope) {
  const epoch = parsePositiveInt(bundle?.epoch, 'settlement bundle epoch');
  if (!isObject(rate) || rate.denom !== 'tap_usd_au') {
    throw new Error('confirmed tap/rate/latest is missing or invalid');
  }
  const tapUsdAu = safeAu(rate.tap_usd_au, 'tap/rate/latest.tap_usd_au').toString();
  const rateTs = parseNonNegativeInt(rate.ts, 'tap/rate/latest.ts');
  const source = String(rate.source ?? '').trim();
  if (!source || source.length > 64) throw new Error('tap/rate/latest.source is invalid');
  if (!isHexBytes(rate.posted_by, 32) || rate.posted_by_role !== 'admin') {
    throw new Error('tap/rate/latest is not admin-posted');
  }
  if (!isHexBytes(admin, 32) || rate.posted_by.toLowerCase() !== admin.toLowerCase()) {
    throw new Error('tap/rate/latest was not posted by the current admin');
  }
  const rateRecordKey = String(rate.updated_at ?? '').trim();
  const expectedPrefix = `rate/tap/${rateTs}/`;
  if (!rateRecordKey.startsWith(expectedPrefix) || !isHexBytes(rateRecordKey.slice(expectedPrefix.length), 32)) {
    throw new Error('tap/rate/latest.updated_at is not a canonical TAP rate evidence key');
  }
  return {
    type: 'tap_settlement_rate_lock',
    epoch,
    bundle_sha256: settlementBundleSha256(bundle),
    denom: 'tap_usd_au',
    tap_usd_au: tapUsdAu,
    source,
    rate_ts: rateTs,
    rate_record_key: rateRecordKey,
    posted_by: rate.posted_by.toLowerCase(),
    posted_by_role: 'admin',
    ...paymentScope,
  };
}

function validateTapRateLock(lock, bundle, paymentScope) {
  if (!isObject(lock)) throw new Error('TAP settlement rate lock must be an object');
  const expectedKeys = [
    'bundle_sha256',
    'chain_id',
    'denom',
    'epoch',
    'payment_config_ver',
    'pool_address',
    'posted_by',
    'posted_by_role',
    'rate_record_key',
    'rate_ts',
    'source',
    'tap_usd_au',
    'token_address',
    'type',
  ];
  const actualKeys = Object.keys(lock).sort();
  if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
    throw new Error('TAP settlement rate lock has an invalid shape');
  }
  if (lock.type !== 'tap_settlement_rate_lock' || lock.denom !== 'tap_usd_au') {
    throw new Error('TAP settlement rate lock type or denomination is invalid');
  }
  const epoch = parsePositiveInt(bundle?.epoch, 'settlement bundle epoch');
  if (lock.epoch !== epoch) throw new Error('TAP settlement rate lock epoch does not match bundle');
  const bundleSha256 = settlementBundleSha256(bundle);
  if (lock.bundle_sha256 !== bundleSha256) {
    throw new Error('TAP settlement rate lock does not match bundle content');
  }
  safeAu(lock.tap_usd_au, 'TAP settlement rate lock tap_usd_au');
  const rateTs = parseNonNegativeInt(lock.rate_ts, 'TAP settlement rate lock rate_ts');
  const expectedPrefix = `rate/tap/${rateTs}/`;
  if (!String(lock.rate_record_key).startsWith(expectedPrefix)
    || !isHexBytes(String(lock.rate_record_key).slice(expectedPrefix.length), 32)) {
    throw new Error('TAP settlement rate lock evidence key is invalid');
  }
  if (!isHexBytes(lock.posted_by, 32) || lock.posted_by_role !== 'admin') {
    throw new Error('TAP settlement rate lock admin evidence is invalid');
  }
  if (typeof lock.source !== 'string' || !lock.source || lock.source.length > 64) {
    throw new Error('TAP settlement rate lock source is invalid');
  }
  const lockedScope = {
    chain_id: parsePositiveInt(lock.chain_id, 'TAP settlement rate lock chain_id'),
    token_address: normalizeAddress(lock.token_address, 'TAP settlement rate lock token_address'),
    pool_address: normalizeAddress(lock.pool_address, 'TAP settlement rate lock pool_address'),
    payment_config_ver: parsePositiveInt(
      lock.payment_config_ver,
      'TAP settlement rate lock payment_config_ver'
    ),
  };
  if (stableJson(lockedScope) !== stableJson(paymentScope)) {
    throw new Error('TAP settlement rate lock does not match the canonical payment pool');
  }
  return lock;
}

function createRateLockFile(lockPath, lock) {
  fs.mkdirSync(path.dirname(lockPath), { recursive: true, mode: 0o700 });
  const tempPath = `${lockPath}.${process.pid}.${crypto.randomBytes(8).toString('hex')}.tmp`;
  fs.writeFileSync(tempPath, `${JSON.stringify(lock, null, 2)}\n`, {
    encoding: 'utf8',
    flag: 'wx',
    mode: 0o600,
  });
  try {
    fs.linkSync(tempPath, lockPath);
  } catch (error) {
    if (error?.code !== 'EEXIST') throw error;
  } finally {
    fs.rmSync(tempPath, { force: true });
  }
}

export async function resolveTapSettlementRate({
  bundle,
  tapRateLockPath,
  peerRpcUrl,
  fetchImpl,
} = {}) {
  if (!tapRateLockPath) {
    throw new Error('TAP settlement requires --tap-rate-lock <path>');
  }
  const lockPath = path.resolve(tapRateLockPath);
  const [payments, admin] = await Promise.all([
    readContractStateValue(peerRpcUrl, 'payments/current', { confirmed: true, fetchImpl }),
    readContractStateValue(peerRpcUrl, 'admin', { confirmed: true, fetchImpl }),
  ]);
  const paymentScope = canonicalTapPaymentScope(payments, admin);
  if (fs.existsSync(lockPath)) {
    return validateTapRateLock(
      readJson(lockPath, 'TAP settlement rate lock'),
      bundle,
      paymentScope
    );
  }
  const rate = await readContractStateValue(
    peerRpcUrl,
    'tap/rate/latest',
    { confirmed: true, fetchImpl }
  );
  createRateLockFile(lockPath, tapRateRecordToLock(rate, bundle, admin, paymentScope));
  return validateTapRateLock(
    readJson(lockPath, 'TAP settlement rate lock'),
    bundle,
    paymentScope
  );
}

function buildReplayCommand({
  bundlePath,
  peerRpcUrl,
  buyerAccountsPath,
  tapRateLockPath,
  ledgerFeeBps,
  priorPath,
  poolAddress,
  operatorAddress,
  epoch,
  confirm,
  json,
} = {}) {
  const args = ['node', 'contracts/scripts/tap-settlement-roller.mjs'];
  if (bundlePath) args.push('--bundle', bundlePath);
  if (peerRpcUrl) args.push('--peer-rpc', peerRpcUrl);
  if (buyerAccountsPath) args.push('--buyer-accounts', buyerAccountsPath);
  if (tapRateLockPath) args.push('--tap-rate-lock', tapRateLockPath);
  if (ledgerFeeBps !== undefined && ledgerFeeBps !== null) args.push('--ledger-fee-bps', String(ledgerFeeBps));
  if (priorPath) args.push('--prior', priorPath);
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
  if (args['tap-usd-au'] !== undefined) {
    throw new Error('--tap-usd-au is not supported; settlement must use --tap-rate-lock backed by confirmed ledger evidence');
  }
  const bundlePath = args.bundle || args['receipts-file'];
  if (!bundlePath) throw new Error('Missing --bundle/--receipts-file.');
  const bundle = readJson(path.resolve(bundlePath), 'receipt bundle');
  if (args['provider-accounts'] !== undefined) {
    throw new Error('--provider-accounts is not supported.');
  }
  for (const key of ['settle-through-epoch', 'challenge-epochs', 'holdback-epochs']) {
    if (args[key] !== undefined) {
      throw new Error(`--${key} is not supported; TAP epoch policy comes from active admin ledger state`);
    }
  }
  const buyerAccountsPath = args['buyer-accounts'];
  const buyerAccounts = buyerAccountsPath
    ? readJson(path.resolve(buyerAccountsPath), 'buyer accounts')
    : {};
  const priorPath = args.prior;
  const prior = priorPath ? readJson(path.resolve(priorPath), 'prior settlement') : null;
  const peerRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const ethRpc = args['eth-rpc'] || args.rpc || process.env.MAYHEM_TAP_ETH_RPC;
  const poolAddress = args.pool || process.env.MAYHEM_TAP_POOL_ADDRESS;
  const operatorAddress = args['operator-address'] || process.env.MAYHEM_TAP_OPERATOR_ADDRESS;
  if (!poolAddress) throw new Error('Missing --pool or MAYHEM_TAP_POOL_ADDRESS.');
  const targetedPayouts = await resolveTargetedTapPayoutsFromLedger({
    bundle,
    peerRpcUrl,
  });
  const epochPolicy = await resolveTapSettlementEpochPolicy({
    peerRpcUrl,
  });
  const tapRateLockPath = args['tap-rate-lock'];
  const tapRateLock = await resolveTapSettlementRate({
    bundle,
    tapRateLockPath,
    peerRpcUrl,
  });
  if (normalizeAddress(poolAddress, 'TAP pool address') !== tapRateLock.pool_address) {
    throw new Error('Configured TAP pool does not match the canonical payment pool');
  }
  const tapUsdAu = tapRateLock.tap_usd_au;
  const ledgerFeeBps = args['ledger-fee-bps'];
  for (const binding of Object.values(targetedPayouts.sessionBindings)) {
    if (binding.chain_id !== tapRateLock.chain_id) {
      throw new Error('Targeted TAP binding chain does not match the settlement pool');
    }
  }

  const confirm = boolArg(args.confirm, false);
  const json = boolArg(args.json, false);
  let pool = null;
  let ownerSigner = null;
  let governanceSigner = null;
  let preparationAdminSigner = null;
  let signerEnvName = null;
  let governanceSignerEnvName = null;
  if (confirm || ethRpc || poolAddress) {
    if (!ethRpc) throw new Error('Missing --eth-rpc or MAYHEM_TAP_ETH_RPC.');
    if (!poolAddress) throw new Error('Missing --pool or MAYHEM_TAP_POOL_ADDRESS.');
    cliEthProvider = new ethers.JsonRpcProvider(ethRpc);
    const signer = tapRollerWallet(cliEthProvider);
    const governance = tapGovernanceWallet(cliEthProvider);
    ownerSigner = signer.wallet;
    governanceSigner = governance.wallet;
    signerEnvName = signer.envName;
    governanceSignerEnvName = governance.envName;
    pool = new ethers.Contract(poolAddress, POOL_SETTLEMENT_ABI, cliEthProvider);
    const [network, poolToken] = await Promise.all([
      cliEthProvider.getNetwork(),
      pool.token(),
    ]);
    if (network.chainId !== BigInt(tapRateLock.chain_id)) {
      throw new Error('Ethereum RPC chain does not match the canonical TAP payment chain');
    }
    if (normalizeAddress(poolToken, 'TAP pool token') !== tapRateLock.token_address) {
      throw new Error('TAP pool token does not match the canonical payment token');
    }
  }
  if (confirm) {
    preparationAdminSigner = await tapPreparationAdminSignerFromEnv();
  }

  const report = await rollTapSettlement({
    bundle,
    targetedSessionBindings: targetedPayouts.sessionBindings,
    canonicalLiabilities: targetedPayouts.liabilities,
    buyerAccounts,
    prior,
    tapUsdAu,
    ledgerFeeBps,
    settleThroughEpoch: epochPolicy.settleThroughEpoch,
    challengeEpochs: epochPolicy.challengeEpochs,
    holdbackEpochs: 0,
    epochApplyHash: bundle.epoch_apply_hash,
    tapRateLock,
    pool,
    ownerSigner,
    governanceSigner,
    operatorAddress,
    epoch: args.epoch ? parsePositiveInt(args.epoch, '--epoch') : undefined,
    canonicalPreparationSubmitter: confirm
      ? ({ plan }) => submitCanonicalTapPreparationPlan({
        plan,
        adminSigner: preparationAdminSigner,
        peerRpcUrl,
      })
      : null,
    post: confirm,
  });
  if (signerEnvName) report.signer_env = signerEnvName;
  if (governanceSignerEnvName) report.governance_signer_env = governanceSignerEnvName;
  report.tap_rate_lock = tapRateLock;
  report.copy_paste_replay_command = buildReplayCommand({
    bundlePath,
    peerRpcUrl,
    buyerAccountsPath,
    tapRateLockPath,
    ledgerFeeBps,
    priorPath,
    poolAddress,
    operatorAddress,
    epoch: report.epoch,
    confirm,
    json: true,
  });
  if (!confirm && report.root && ethRpc && poolAddress) {
    report.copy_paste_confirm_command = buildReplayCommand({
      bundlePath,
      peerRpcUrl,
      buyerAccountsPath,
      tapRateLockPath,
      ledgerFeeBps,
      priorPath,
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
    if (report.propose_root_calldata) console.log('[tap:settlement] proposeRoot calldata:', report.propose_root_calldata);
    if (report.propose_root_dry_run) {
      console.log('[tap:settlement] proposeRoot dry-run:', JSON.stringify(report.propose_root_dry_run));
    }
    if (report.execute_root_dry_run) {
      console.log('[tap:settlement] executeRoot dry-run:', JSON.stringify(report.execute_root_dry_run));
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
    console.error(safeErrorMessage(error));
    process.exit(1);
  });
}
