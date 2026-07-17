#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { safeErrorMessage } from './safe-output.mjs';

const scriptPath = fileURLToPath(import.meta.url);
const DEFAULT_CURSOR = path.resolve('.mayhem-local', 'tap-deposit-watcher.json');
const TAP_WEI = 1_000_000_000_000_000_000n;
const MIN_TAP_CONFIRMATIONS = 12;
const DEFAULT_LOOKBACK_BLOCKS = 50_000;
const DEFAULT_RETRY_ATTEMPTS = 5;
const DEFAULT_RETRY_BASE_MS = 250;
const DEFAULT_RETRY_MAX_MS = 4_000;
export const TAP_DEPOSIT_EVENT_SIGNATURE = ethers.id('Deposit(address,uint256)');
export const TAP_DEPOSIT_WATCHER_ID = 'tap-deposit-watcher-v1';

export const POOL_ABI = [
  'event Deposit(address indexed buyer, uint256 amount)',
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

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '')).toLowerCase();
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function normalizeTxHash(value) {
  const hash = String(value ?? '').trim().toLowerCase();
  if (!/^0x[0-9a-f]{64}$/.test(hash)) throw new Error('eth_tx_hash must be a 32-byte 0x hex string');
  return hash;
}

function normalizeBlockHash(value) {
  const hash = String(value ?? '').trim().toLowerCase();
  if (!/^0x[0-9a-f]{64}$/.test(hash)) throw new Error('block_hash must be a 32-byte 0x hex string');
  return hash;
}

export function redactRpcUrl(value) {
  try {
    const url = new URL(String(value));
    if (url.username) url.username = '***';
    if (url.password) url.password = '***';
    if (url.search) url.search = '';
    if ((url.pathname || '').length > 1) url.pathname = '/...';
    return url.toString();
  } catch (_error) {
    return '<redacted>';
  }
}

function positiveDecimalBigInt(value, label) {
  try {
    const raw = String(value ?? '').trim();
    if (!/^(0|[1-9]\d*)$/.test(raw)) throw new Error();
    const parsed = BigInt(raw);
    if (parsed <= 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a positive decimal integer`);
  }
}

export function tapWeiToAu(tapWei, tapUsdAu) {
  const wei = positiveDecimalBigInt(tapWei, 'tap_wei');
  const rate = positiveDecimalBigInt(tapUsdAu, 'tap_usd_au');
  return ((wei * rate) / TAP_WEI).toString();
}

export function tapDepositFromLog(log, {
  chainId,
  poolAddress,
  tapUsdAu,
  finalizedBlockNumber,
  confirmationDepth,
  confirmationPolicy = 'depth',
  watcherId = TAP_DEPOSIT_WATCHER_ID,
} = {}) {
  const buyer = log.args?.buyer ?? log.args?.[0];
  const amount = log.args?.amount ?? log.args?.[1];
  const logIndex = Number(log.index ?? log.logIndex);
  const blockNumber = Number(log.blockNumber);
  if (!Number.isSafeInteger(logIndex) || logIndex < 0) throw new Error('Deposit log has invalid log_index');
  if (!Number.isSafeInteger(blockNumber) || blockNumber < 0) throw new Error('Deposit log has invalid block_number');
  const safeFinalizedBlockNumber = parseNonNegativeInt(finalizedBlockNumber ?? blockNumber, 'finalized_block_number');
  const safeConfirmationDepth = parseNonNegativeInt(
    confirmationDepth ?? Math.max(0, safeFinalizedBlockNumber - blockNumber),
    'confirmation_depth'
  );
  const normalizedChainId = parsePositiveInt(chainId, 'chain_id');
  const tapWei = positiveDecimalBigInt(amount, 'tap_wei').toString();
  const normalizedRate = tapUsdAu === undefined || tapUsdAu === null || tapUsdAu === ''
    ? null
    : positiveDecimalBigInt(tapUsdAu, 'tap_usd_au').toString();
  const deposit = {
    who: normalizeAddress(buyer, 'buyer'),
    tap_wei: tapWei,
    eth_tx_hash: normalizeTxHash(log.transactionHash),
    log_index: logIndex,
    block_number: blockNumber,
    block_hash: normalizeBlockHash(log.blockHash),
    pool_address: normalizeAddress(poolAddress ?? log.address, 'pool_address'),
    chain_id: normalizedChainId,
    finalized_block_number: safeFinalizedBlockNumber,
    confirmation_depth: safeConfirmationDepth,
    confirmation_policy: String(confirmationPolicy),
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: String(watcherId),
  };
  if (normalizedRate !== null) {
    deposit.tap_usd_au = normalizedRate;
    deposit.au = tapWeiToAu(tapWei, normalizedRate);
  }
  return deposit;
}

export function tapDepositKey(deposit) {
  const chainId = parsePositiveInt(deposit.chain_id, 'chain_id');
  const poolAddress = normalizeAddress(deposit.pool_address, 'pool_address');
  const ethTxHash = normalizeTxHash(deposit.eth_tx_hash);
  const logIndex = parseNonNegativeInt(deposit.log_index, 'log_index');
  const blockHash = normalizeBlockHash(deposit.block_hash);
  return `${chainId}/${poolAddress}/${ethTxHash}/${logIndex}/${blockHash}`;
}

export function tapDepositReversalKey(deposit) {
  return `${tapDepositKey(deposit)}/reversal`;
}

export function tapDepositReversalFromCredit(deposit, {
  reconciliationFromBlock,
  reconciliationToBlock,
  finalizedBlockNumber,
  confirmationPolicy = 'finalized-tag',
  watcherId = TAP_DEPOSIT_WATCHER_ID,
} = {}) {
  if (confirmationPolicy !== 'finalized-tag') {
    throw new Error('TAP deposit reversal requires finalized-tag reconciliation');
  }
  const blockNumber = parseNonNegativeInt(deposit.block_number, 'block_number');
  const from = parseNonNegativeInt(reconciliationFromBlock, 'reconciliation_from_block');
  const to = parseNonNegativeInt(reconciliationToBlock, 'reconciliation_to_block');
  const finalized = parseNonNegativeInt(finalizedBlockNumber, 'finalized_block_number');
  if (from > blockNumber || to < blockNumber) {
    throw new Error('TAP reversal reconciliation window must contain the credited block');
  }
  if (finalized - to < MIN_TAP_CONFIRMATIONS) {
    throw new Error(`TAP reversal requires a ${MIN_TAP_CONFIRMATIONS}-block finalized gap`);
  }
  return {
    chain_id: parsePositiveInt(deposit.chain_id, 'chain_id'),
    pool_address: normalizeAddress(deposit.pool_address, 'pool_address'),
    eth_tx_hash: normalizeTxHash(deposit.eth_tx_hash),
    log_index: parseNonNegativeInt(deposit.log_index, 'log_index'),
    block_number: blockNumber,
    block_hash: normalizeBlockHash(deposit.block_hash),
    reconciliation_from_block: from,
    reconciliation_to_block: to,
    finalized_block_number: finalized,
    confirmation_policy: confirmationPolicy,
    watcher_id: String(watcherId),
    reason: 'canonical_event_missing',
  };
}

export function reconcileCreditedDeposits(creditedDeposits, canonicalDeposits, scan) {
  if (scan?.finalizedPolicy !== true) {
    throw new Error('TAP credited-deposit reconciliation requires finalized-tag scan evidence');
  }
  const canonicalKeys = new Set(canonicalDeposits.map(tapDepositKey));
  return creditedDeposits
    .filter((deposit) => {
      const blockNumber = Number(deposit?.block_number);
      return Number.isSafeInteger(blockNumber)
        && blockNumber >= scan.from
        && blockNumber <= scan.to
        && !canonicalKeys.has(tapDepositKey(deposit));
    })
    .map((deposit) => tapDepositReversalFromCredit(deposit, {
      reconciliationFromBlock: scan.from,
      reconciliationToBlock: scan.to,
      finalizedBlockNumber: scan.referenceBlock,
      confirmationPolicy: 'finalized-tag',
    }));
}

export function buildAdminCommandArgs(deposit, {
  epoch,
  at,
  submit = true,
  sim = false,
  json = true,
  rpcUrl,
  home,
  peerStoreName,
  walletPassword,
} = {}) {
  const args = [
    'admin',
    'tap-deposit',
    '--who',
    deposit.who,
    '--tap-wei',
    deposit.tap_wei,
    '--eth-tx-hash',
    deposit.eth_tx_hash,
    '--log-index',
    String(deposit.log_index),
    '--block-number',
    String(deposit.block_number),
    '--block-hash',
    deposit.block_hash,
    '--pool-address',
    deposit.pool_address,
    '--chain-id',
    String(deposit.chain_id),
    '--finalized-block-number',
    String(deposit.finalized_block_number),
    '--confirmation-depth',
    String(deposit.confirmation_depth),
    '--confirmation-policy',
    deposit.confirmation_policy,
    '--event-signature',
    deposit.event_signature,
    '--watcher-id',
    deposit.watcher_id,
    '--epoch',
    String(epoch),
    '--at',
    String(at),
  ];
  if (home) args.push('--home', home);
  if (rpcUrl) args.push('--rpc-url', rpcUrl);
  if (peerStoreName) args.push('--peer-store-name', peerStoreName);
  if (walletPassword) args.push('--wallet-password', walletPassword);
  if (submit) args.push('--submit');
  if (sim) args.push('--sim');
  if (json) args.push('--json');
  return args;
}

export function buildAdminCommand(deposit, options = {}) {
  const bin = options.mayhemBin ?? 'mayhem';
  const args = buildAdminCommandArgs(deposit, options)
    .filter((arg, index, all) => all[index - 1] !== '--wallet-password' && arg !== '--wallet-password');
  return [bin, ...args].map(shellQuote).join(' ');
}

export function buildAdminReversalCommandArgs(reversal, {
  epoch,
  at,
  submit = true,
  sim = false,
  json = true,
  rpcUrl,
  home,
  peerStoreName,
  walletPassword,
} = {}) {
  const args = [
    'admin',
    'tap-deposit-reversal',
    '--eth-tx-hash',
    reversal.eth_tx_hash,
    '--log-index',
    String(reversal.log_index),
    '--block-number',
    String(reversal.block_number),
    '--block-hash',
    reversal.block_hash,
    '--pool-address',
    reversal.pool_address,
    '--chain-id',
    String(reversal.chain_id),
    '--reconciliation-from-block',
    String(reversal.reconciliation_from_block),
    '--reconciliation-to-block',
    String(reversal.reconciliation_to_block),
    '--finalized-block-number',
    String(reversal.finalized_block_number),
    '--confirmation-policy',
    reversal.confirmation_policy,
    '--watcher-id',
    reversal.watcher_id,
    '--reason',
    reversal.reason,
    '--epoch',
    String(epoch),
    '--at',
    String(at),
  ];
  if (home) args.push('--home', home);
  if (rpcUrl) args.push('--rpc-url', rpcUrl);
  if (peerStoreName) args.push('--peer-store-name', peerStoreName);
  if (walletPassword) args.push('--wallet-password', walletPassword);
  if (submit) args.push('--submit');
  if (sim) args.push('--sim');
  if (json) args.push('--json');
  return args;
}

export function buildAdminReversalCommand(reversal, options = {}) {
  const bin = options.mayhemBin ?? 'mayhem';
  const args = buildAdminReversalCommandArgs(reversal, options)
    .filter((arg, index, all) => all[index - 1] !== '--wallet-password' && arg !== '--wallet-password');
  return [bin, ...args].map(shellQuote).join(' ');
}

function ensureRpcBase(url) {
  const raw = String(url ?? '').trim();
  if (!raw) throw new Error('Missing peer RPC URL');
  return raw.endsWith('/') ? raw : `${raw}/`;
}

async function fetchJson(url, fetchImpl = globalThis.fetch) {
  const response = await fetchImpl(url);
  if (!response.ok) throw new Error(`GET ${url} failed with ${response.status}`);
  return await response.json();
}

export async function readContractStateValue(rpcUrl, key, {
  confirmed = false,
  fetchImpl = globalThis.fetch,
} = {}) {
  const url = new URL('state', ensureRpcBase(rpcUrl));
  url.searchParams.set('key', key);
  url.searchParams.set('confirmed', confirmed ? 'true' : 'false');
  const body = await fetchJson(url, fetchImpl);
  return body?.value ?? null;
}

export async function resolveActiveBillingEpoch(explicitEpoch, rpcUrl, {
  fetchImpl = globalThis.fetch,
} = {}) {
  if (explicitEpoch !== undefined && explicitEpoch !== null && explicitEpoch !== '') {
    return parsePositiveInt(explicitEpoch, '--epoch');
  }
  if (!rpcUrl) throw new Error('Missing --epoch or --peer-rpc for active billing epoch discovery.');
  const applyState = await readContractStateValue(rpcUrl, 'epoch/apply/state', { fetchImpl });
  const updatedEpoch = applyState?.updated_epoch ?? 0;
  if (!Number.isSafeInteger(updatedEpoch) || updatedEpoch < 0) {
    throw new Error('epoch/apply/state.updated_epoch must be a non-negative safe integer');
  }
  const epoch = updatedEpoch + 1;
  if (!Number.isSafeInteger(epoch) || epoch <= 0) {
    throw new Error('active billing epoch overflowed');
  }
  return epoch;
}

export function tapDepositStateMatches(deposit, {
  seen,
  balance,
  depositRoot,
  epoch,
} = {}) {
  const readAu = (value) => {
    try {
      const parsed = BigInt(String(value ?? '').trim());
      return parsed >= 0n ? parsed : null;
    } catch (_error) {
      return null;
    }
  };
  const balanceAu = readAu(balance?.au);
  const rootAu = readAu(depositRoot?.au_total);
  const rootCount = Number(depositRoot?.count);
  const expectedAu = readAu(deposit?.au);
  const hasExpectedAu = expectedAu !== null && expectedAu > 0n;
  const canonicalWho = seen?.who;
  return seen !== null
    && seen?.eth_tx_hash === deposit.eth_tx_hash
    && Number(seen?.log_index) === deposit.log_index
    && seen?.ethereum_address === deposit.who
    && seen?.tap_wei === deposit.tap_wei
    && Number(seen?.block_number) === Number(deposit.block_number)
    && seen?.block_hash === deposit.block_hash
    && Number(seen?.chain_id) === Number(deposit.chain_id)
    && seen?.pool_address === deposit.pool_address
    && Number(seen?.finalized_block_number) === Number(deposit.finalized_block_number)
    && Number(seen?.confirmation_depth) === Number(deposit.confirmation_depth)
    && seen?.confirmation_policy === deposit.confirmation_policy
    && seen?.event_signature === deposit.event_signature
    && seen?.watcher_id === deposit.watcher_id
    && (!hasExpectedAu || readAu(seen?.au) === expectedAu)
    && Number(seen?.epoch) === Number(epoch)
    && seen?.reversed !== true
    && typeof canonicalWho === 'string'
    && balance?.user === canonicalWho
    && balance?.rail === 'tap'
    && balance?.denom === 'au_usd'
    && balanceAu !== null
    && depositRoot?.type === 'deposit_root'
    && Number(depositRoot?.epoch) === Number(epoch)
    && Number.isSafeInteger(rootCount)
    && rootCount > 0
    && rootAu !== null
    && (!hasExpectedAu || rootAu >= expectedAu);
}

export async function waitForTapDepositState(deposit, {
  rpcUrl,
  epoch,
  timeoutMs = 0,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = timeoutMs > 0 ? Date.now() + timeoutMs : null;
  const seenKey = `dep/tap/${tapDepositKey(deposit)}`;
  let state = null;
  while (deadline === null || Date.now() <= deadline) {
    const [seen, depositRoot] = await Promise.all([
      readContractStateValue(rpcUrl, seenKey, { fetchImpl }),
      readContractStateValue(rpcUrl, `ev/dep/${epoch}`, { fetchImpl }),
    ]);
    const balanceWho = seen?.who ?? deposit.who;
    const balance = await readContractStateValue(rpcUrl, `bal/${balanceWho}/tap`, { fetchImpl });
    state = { seen, balance, depositRoot };
    if (tapDepositStateMatches(deposit, { ...state, epoch })) {
      return { verified: true, state };
    }
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }
  return { verified: false, state };
}

export function tapDepositReversalStateMatches(reversal, {
  seen,
  reversalSeen,
  depositRoot,
  epoch,
} = {}) {
  return seen?.reversed === true
    && seen?.eth_tx_hash === reversal.eth_tx_hash
    && Number(seen?.log_index) === reversal.log_index
    && Number(seen?.block_number) === reversal.block_number
    && seen?.block_hash === reversal.block_hash
    && reversalSeen?.reason === 'canonical_event_missing'
    && reversalSeen?.eth_tx_hash === reversal.eth_tx_hash
    && Number(reversalSeen?.log_index) === reversal.log_index
    && Number(reversalSeen?.block_number) === reversal.block_number
    && reversalSeen?.block_hash === reversal.block_hash
    && Number(reversalSeen?.reconciliation_from_block) === reversal.reconciliation_from_block
    && Number(reversalSeen?.reconciliation_to_block) === reversal.reconciliation_to_block
    && Number(reversalSeen?.finalized_block_number) === reversal.finalized_block_number
    && reversalSeen?.confirmation_policy === 'finalized-tag'
    && reversalSeen?.watcher_id === TAP_DEPOSIT_WATCHER_ID
    && depositRoot?.type === 'deposit_root'
    && Number(depositRoot?.epoch) === Number(epoch);
}

export async function waitForTapDepositReversalState(reversal, {
  rpcUrl,
  epoch,
  timeoutMs = 0,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = timeoutMs > 0 ? Date.now() + timeoutMs : null;
  const seenKey = `dep/tap/${tapDepositKey(reversal)}`;
  let state = null;
  while (deadline === null || Date.now() <= deadline) {
    const [seen, reversalSeen, depositRoot] = await Promise.all([
      readContractStateValue(rpcUrl, seenKey, { fetchImpl }),
      readContractStateValue(rpcUrl, `${seenKey}/reversal`, { fetchImpl }),
      readContractStateValue(rpcUrl, `ev/dep/${epoch}`, { fetchImpl }),
    ]);
    state = { seen, reversalSeen, depositRoot };
    if (tapDepositReversalStateMatches(reversal, { ...state, epoch })) {
      return { verified: true, state };
    }
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }
  return { verified: false, state };
}

function readJsonIfExists(filePath, fallback) {
  if (!filePath || !fs.existsSync(filePath)) return fallback;
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function normalizeDepositList(value) {
  if (!Array.isArray(value)) return [];
  const deposits = [];
  for (const deposit of value) {
    try {
      tapDepositKey(deposit);
      deposits.push(deposit);
    } catch (_error) {}
  }
  return deposits;
}

function normalizeReversalList(value) {
  if (!Array.isArray(value)) return [];
  const reversals = [];
  for (const reversal of value) {
    try {
      tapDepositReversalKey(reversal);
      if (reversal.reason !== 'canonical_event_missing') continue;
      reversals.push(reversal);
    } catch (_error) {}
  }
  return reversals;
}

function normalizeCursor(raw) {
  const cursor = raw && typeof raw === 'object' ? raw : {};
  return {
    next_block: Number.isSafeInteger(cursor.next_block) && cursor.next_block >= 0
      ? cursor.next_block
      : 0,
    credited_deposits: normalizeDepositList(cursor.credited_deposits),
    pending_deposits: normalizeDepositList(cursor.pending_deposits),
    pending_reversals: normalizeReversalList(cursor.pending_reversals),
    reversed_log_keys: Array.isArray(cursor.reversed_log_keys)
      ? cursor.reversed_log_keys.filter((key) => typeof key === 'string')
      : [],
  };
}

export function adminSubmitFailureIsTransient(child) {
  if (child?.status === 0) return false;
  if (child?.status === null || child?.signal) return true;
  const output = `${child?.stdout ?? ''}\n${child?.stderr ?? ''}`.toLowerCase();
  return [
    /rate oracle timestamp is in the future/,
    /econn(?:reset|refused|aborted)/,
    /connection (?:reset|refused|closed|lost)/,
    /socket hang up/,
    /network (?:error|unavailable|unreachable)/,
    /timed? out/,
    /timeout/,
    /\b429\b/,
    /\b50[0234]\b/,
    /temporar(?:y|ily)/,
    /resource .*unavailable/,
    /peer .*not (?:ready|available|connected)/,
  ].some((pattern) => pattern.test(output));
}

export async function runAdminCommandWithRetry({
  mayhemBin,
  buildArgs,
  attempts = DEFAULT_RETRY_ATTEMPTS,
  baseDelayMs = DEFAULT_RETRY_BASE_MS,
  maxDelayMs = DEFAULT_RETRY_MAX_MS,
  spawnImpl = spawnSync,
  sleep = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)),
} = {}) {
  const maxAttempts = parsePositiveInt(attempts, '--retry-attempts', DEFAULT_RETRY_ATTEMPTS);
  const base = parseNonNegativeInt(baseDelayMs, '--retry-base-ms', DEFAULT_RETRY_BASE_MS);
  const cap = parseNonNegativeInt(maxDelayMs, '--retry-max-ms', DEFAULT_RETRY_MAX_MS);
  let child;
  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    child = spawnImpl(mayhemBin, buildArgs(attempt), { encoding: 'utf8' });
    if (child.status === 0 || !adminSubmitFailureIsTransient(child) || attempt + 1 === maxAttempts) {
      return { child, attempts: attempt + 1, transient: adminSubmitFailureIsTransient(child) };
    }
    await sleep(Math.min(cap, base * (2 ** attempt)));
  }
  return { child, attempts: maxAttempts, transient: adminSubmitFailureIsTransient(child) };
}

async function safeToBlock(provider, { toBlock, confirmations = 12, blockTag, chainId }) {
  const effectiveBlockTag = blockTag ?? (Number(chainId) === 1 ? 'finalized' : undefined);
  if (Number(chainId) === 1 && effectiveBlockTag !== 'finalized') {
    throw new Error('Ethereum mainnet TAP watcher requires --block-tag finalized');
  }
  if (effectiveBlockTag === 'finalized') {
    const block = await provider.send('eth_getBlockByNumber', ['finalized', false]);
    if (!block || block.number == null) throw new Error('finalized block tag unavailable from RPC');
    const referenceBlock = Number(block.number);
    const maxSafe = Math.max(-1, referenceBlock - MIN_TAP_CONFIRMATIONS);
    if (toBlock !== undefined && toBlock !== null && toBlock !== '') {
      const explicit = parseNonNegativeInt(toBlock, '--to-block');
      if (explicit > maxSafe) {
        throw new Error(`--to-block must be at least ${MIN_TAP_CONFIRMATIONS} blocks behind the finalized reference`);
      }
      return { safeTo: explicit, referenceBlock, finalizedPolicy: true };
    }
    return { safeTo: maxSafe, referenceBlock, finalizedPolicy: true };
  }
  const head = Number(await provider.send('eth_blockNumber', []));
  const depth = parseNonNegativeInt(confirmations, '--confirmations', 12);
  if (depth < MIN_TAP_CONFIRMATIONS) {
    throw new Error(`--confirmations must be at least ${MIN_TAP_CONFIRMATIONS}`);
  }
  const maxSafe = Math.max(-1, head - depth);
  if (toBlock !== undefined && toBlock !== null && toBlock !== '') {
    const explicit = parseNonNegativeInt(toBlock, '--to-block');
    if (explicit > maxSafe) {
      throw new Error(`--to-block must be at least ${MIN_TAP_CONFIRMATIONS} confirmations deep`);
    }
    return { safeTo: explicit, referenceBlock: head, finalizedPolicy: false };
  }
  return { safeTo: maxSafe, referenceBlock: head, finalizedPolicy: false };
}

export async function scanTapDeposits({
  pool,
  fromBlock = 0,
  toBlock,
  confirmations = 12,
  blockTag,
  chunkSize = 5000,
  lookbackBlocks = DEFAULT_LOOKBACK_BLOCKS,
  tapUsdAu,
  chainId,
  poolAddress,
} = {}) {
  const provider = pool.runner?.provider ?? pool.runner ?? pool.provider;
  if (!provider) throw new Error('pool contract has no provider');
  const network = chainId ? null : await provider.getNetwork();
  const normalizedChainId = Number(chainId ?? network.chainId);
  const { safeTo, referenceBlock, finalizedPolicy } = await safeToBlock(provider, {
    toBlock,
    confirmations,
    blockTag,
    chainId: normalizedChainId,
  });
  const requestedFrom = parseNonNegativeInt(fromBlock, '--from-block', 0);
  const lookback = parsePositiveInt(lookbackBlocks, '--lookback-blocks', DEFAULT_LOOKBACK_BLOCKS);
  const from = Math.max(0, requestedFrom - (lookback - 1));
  if (safeTo < from) {
    return {
      deposits: [],
      from,
      to: from - 1,
      referenceBlock,
      finalizedPolicy,
    };
  }

  const deposits = [];
  const window = Math.max(1, parsePositiveInt(chunkSize, '--chunk-size', 5000));
  for (let start = from; start <= safeTo; start += window) {
    const end = Math.min(start + window - 1, safeTo);
    const events = await pool.queryFilter(pool.filters.Deposit(), start, end);
    for (const event of events) {
      const confirmationDepth = Math.max(0, referenceBlock - Number(event.blockNumber));
      deposits.push(tapDepositFromLog(event, {
        chainId: normalizedChainId,
        poolAddress: poolAddress ?? event.address ?? await pool.getAddress(),
        tapUsdAu,
        finalizedBlockNumber: referenceBlock,
        confirmationDepth,
        confirmationPolicy: finalizedPolicy
          ? 'finalized-tag'
          : `depth-${confirmationDepth}`,
      }));
    }
  }
  return { deposits, from, to: safeTo, referenceBlock, finalizedPolicy };
}

export function mergeDeposits(existing, incoming) {
  const merged = new Map();
  for (const deposit of [...existing, ...incoming]) {
    try {
      merged.set(tapDepositKey(deposit), deposit);
    } catch (_error) {}
  }
  return Array.from(merged.values())
    .sort((a, b) => (
      Number(a.block_number) - Number(b.block_number)
      || Number(a.log_index) - Number(b.log_index)
      || String(a.eth_tx_hash).localeCompare(String(b.eth_tx_hash))
    ));
}

export function mergeReversals(existing, incoming) {
  const merged = new Map();
  for (const reversal of [...existing, ...incoming]) {
    try {
      merged.set(tapDepositReversalKey(reversal), reversal);
    } catch (_error) {}
  }
  return Array.from(merged.values())
    .sort((a, b) => (
      Number(a.block_number) - Number(b.block_number)
      || Number(a.log_index) - Number(b.log_index)
      || String(a.eth_tx_hash).localeCompare(String(b.eth_tx_hash))
    ));
}

async function main() {
  const args = parseArgs();
  const rpc = String(args.rpc || process.env.MAYHEM_TAP_ETH_RPC || '').trim();
  const poolAddress = String(args.pool || process.env.MAYHEM_TAP_POOL_ADDRESS || '').trim();
  if (!rpc) throw new Error('Missing --rpc or MAYHEM_TAP_ETH_RPC.');
  if (!poolAddress) throw new Error('Missing --pool or MAYHEM_TAP_POOL_ADDRESS.');

  const tapUsdAu = args['tap-usd-au'] || process.env.MAYHEM_TAP_USD_AU
    ? positiveDecimalBigInt(args['tap-usd-au'] || process.env.MAYHEM_TAP_USD_AU, '--tap-usd-au').toString()
    : undefined;
  const adminRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const epoch = await resolveActiveBillingEpoch(args.epoch, adminRpcUrl);
  const explicitAt = args.at === undefined
    ? null
    : parseNonNegativeInt(args.at, '--at');
  const cursorPath = path.resolve(args.cursor || DEFAULT_CURSOR);
  const cursor = normalizeCursor(readJsonIfExists(cursorPath, {}));
  const fromBlock = args['from-block'] !== undefined
    ? parseNonNegativeInt(args['from-block'], '--from-block')
    : cursor.next_block;
  const confirmations = parseNonNegativeInt(args.confirmations ?? 12, '--confirmations', 12);
  const chunkSize = parsePositiveInt(args['chunk-size'] ?? 5000, '--chunk-size', 5000);
  const lookbackBlocks = parsePositiveInt(
    args['lookback-blocks'] ?? DEFAULT_LOOKBACK_BLOCKS,
    '--lookback-blocks',
    DEFAULT_LOOKBACK_BLOCKS
  );
  const retryAttempts = parsePositiveInt(
    args['retry-attempts'] ?? DEFAULT_RETRY_ATTEMPTS,
    '--retry-attempts',
    DEFAULT_RETRY_ATTEMPTS
  );
  const retryBaseMs = parseNonNegativeInt(
    args['retry-base-ms'] ?? DEFAULT_RETRY_BASE_MS,
    '--retry-base-ms',
    DEFAULT_RETRY_BASE_MS
  );
  const retryMaxMs = parseNonNegativeInt(
    args['retry-max-ms'] ?? DEFAULT_RETRY_MAX_MS,
    '--retry-max-ms',
    DEFAULT_RETRY_MAX_MS
  );
  const submit = boolArg(args.submit, false);
  const sim = boolArg(args.sim, false);
  const json = boolArg(args.json, false);
  const verify = !boolArg(args['no-verify'], false);
  const verifyTimeoutMs = parseNonNegativeInt(args['verify-timeout-ms'] ?? 0, '--verify-timeout-ms', 0);
  const verifyPollMs = parseNonNegativeInt(args['verify-poll-ms'] ?? 500, '--verify-poll-ms', 500);

  const provider = new ethers.JsonRpcProvider(rpc);
  const pool = new ethers.Contract(poolAddress, POOL_ABI, provider);
  const scan = await scanTapDeposits({
    pool,
    fromBlock,
    toBlock: args['to-block'],
    confirmations,
    blockTag: args['block-tag'],
    chunkSize,
    lookbackBlocks,
    tapUsdAu,
    chainId: args['chain-id'] ? parsePositiveInt(args['chain-id'], '--chain-id') : undefined,
    poolAddress,
  });

  const canonicalKeys = new Set(scan.deposits.map(tapDepositKey));
  const reversedKeys = new Set(cursor.reversed_log_keys);
  let creditedDeposits = mergeDeposits([], cursor.credited_deposits);
  const creditedKeys = new Set(creditedDeposits.map(tapDepositKey));
  const detectedReversals = scan.finalizedPolicy
    ? reconcileCreditedDeposits(creditedDeposits, scan.deposits, scan)
    : [];
  const pendingReversals = mergeReversals(cursor.pending_reversals, detectedReversals)
    .filter((reversal) => (
      !canonicalKeys.has(tapDepositKey(reversal))
      && !reversedKeys.has(tapDepositReversalKey(reversal))
    ));
  const pendingDeposits = mergeDeposits(cursor.pending_deposits, scan.deposits)
    .filter((deposit) => (
      (Number(deposit.block_number) < scan.from
        || Number(deposit.block_number) > scan.to
        || canonicalKeys.has(tapDepositKey(deposit)))
      &&
      !creditedKeys.has(tapDepositKey(deposit))
      && !reversedKeys.has(tapDepositReversalKey(deposit))
    ));
  const adminOptions = {
    epoch,
    submit: true,
    sim,
    json: true,
    mayhemBin: args['mayhem-bin'] || 'mayhem',
    rpcUrl: adminRpcUrl,
    home: args['admin-home'],
    peerStoreName: args['admin-peer-store-name'],
    walletPassword: args['admin-wallet-password-env']
      ? process.env[String(args['admin-wallet-password-env'])]
      : undefined,
  };

  const reversalResults = [];
  const reversedThisRun = new Set();
  let submissionBlocked = false;
  for (const reversal of pendingReversals) {
    let submitOptions = {
      ...adminOptions,
      at: explicitAt ?? Math.floor(Date.now() / 1000),
    };
    const result = {
      ...reversal,
      at: submitOptions.at,
      copy_paste_admin_submit_command: buildAdminReversalCommand(reversal, submitOptions),
      submitted: false,
      preflighted: false,
      verified: false,
    };
    if (submit) {
      const execution = await runAdminCommandWithRetry({
        mayhemBin: args['mayhem-bin'] || 'mayhem',
        attempts: retryAttempts,
        baseDelayMs: retryBaseMs,
        maxDelayMs: retryMaxMs,
        buildArgs: () => {
          submitOptions = {
            ...submitOptions,
            at: explicitAt ?? Math.floor(Date.now() / 1000),
          };
          return buildAdminReversalCommandArgs(reversal, submitOptions);
        },
      });
      const { child } = execution;
      result.at = submitOptions.at;
      result.copy_paste_admin_submit_command = buildAdminReversalCommand(reversal, submitOptions);
      result.attempts = execution.attempts;
      if (sim) {
        result.preflighted = child.status === 0;
      } else {
        result.submitted = child.status === 0;
      }
      result.exit_status = child.status;
      result.stdout = child.stdout?.trim() || null;
      result.stderr = child.stderr?.trim() || null;
      if (child.status !== 0) {
        result.failure_kind = execution.transient ? 'transient_exhausted' : 'permanent';
        result.error = `mayhem admin tap-deposit-reversal exited ${child.status}`;
        submissionBlocked = true;
      } else if (!sim) {
        if (adminOptions.rpcUrl && verify) {
          const verification = await waitForTapDepositReversalState(reversal, {
            rpcUrl: adminOptions.rpcUrl,
            epoch,
            timeoutMs: verifyTimeoutMs,
            pollMs: verifyPollMs,
          });
          result.verified = verification.verified;
          result.state = verification.state;
          if (verification.verified) {
            reversedThisRun.add(tapDepositReversalKey(reversal));
          } else {
            result.error = 'tapDepositReversal did not update the credited event, reversal marker, and deposit root';
            submissionBlocked = true;
          }
        } else {
          result.verification_skipped = true;
          reversedThisRun.add(tapDepositReversalKey(reversal));
        }
      }
    }
    reversalResults.push(result);
    if (submissionBlocked) break;
  }

  const depositResults = [];
  const creditedThisRun = new Set();
  if (!submissionBlocked) {
    for (const deposit of pendingDeposits) {
      let submitOptions = {
        ...adminOptions,
        at: explicitAt ?? Math.floor(Date.now() / 1000),
      };
      const result = {
        ...deposit,
        at: submitOptions.at,
        copy_paste_admin_submit_command: buildAdminCommand(deposit, submitOptions),
        submitted: false,
        preflighted: false,
        verified: false,
      };
      if (submit) {
        const execution = await runAdminCommandWithRetry({
          mayhemBin: args['mayhem-bin'] || 'mayhem',
          attempts: retryAttempts,
          baseDelayMs: retryBaseMs,
          maxDelayMs: retryMaxMs,
          buildArgs: () => {
            submitOptions = {
              ...submitOptions,
              at: explicitAt ?? Math.floor(Date.now() / 1000),
            };
            return buildAdminCommandArgs(deposit, submitOptions);
          },
        });
        const { child } = execution;
        result.at = submitOptions.at;
        result.copy_paste_admin_submit_command = buildAdminCommand(deposit, submitOptions);
        result.attempts = execution.attempts;
        if (sim) {
          result.preflighted = child.status === 0;
        } else {
          result.submitted = child.status === 0;
        }
        result.exit_status = child.status;
        result.stdout = child.stdout?.trim() || null;
        result.stderr = child.stderr?.trim() || null;
        if (child.status !== 0) {
          result.failure_kind = execution.transient ? 'transient_exhausted' : 'permanent';
          result.error = `mayhem admin tap-deposit exited ${child.status}`;
          submissionBlocked = true;
        } else if (!sim) {
          if (adminOptions.rpcUrl && verify) {
            const verification = await waitForTapDepositState(deposit, {
              rpcUrl: adminOptions.rpcUrl,
              epoch,
              timeoutMs: verifyTimeoutMs,
              pollMs: verifyPollMs,
            });
            result.verified = verification.verified;
            result.state = verification.state;
            if (verification.verified) {
              creditedThisRun.add(tapDepositKey(deposit));
            } else {
              result.error = 'tapDeposit submit did not update the seen event, balance, and deposit root';
              submissionBlocked = true;
            }
          } else {
            result.verification_skipped = true;
            creditedThisRun.add(tapDepositKey(deposit));
          }
        }
      }
      depositResults.push(result);
      if (submissionBlocked) break;
    }
  }

  for (const key of reversedThisRun) reversedKeys.add(key);
  creditedDeposits = creditedDeposits.filter(
    (deposit) => !reversedKeys.has(tapDepositReversalKey(deposit))
  );
  creditedDeposits = mergeDeposits(
    creditedDeposits,
    pendingDeposits.filter((deposit) => creditedThisRun.has(tapDepositKey(deposit)))
  );
  const finalCreditedKeys = new Set(creditedDeposits.map(tapDepositKey));
  const nextCursor = {
    next_block: Math.max(cursor.next_block, scan.to + 1),
    credited_deposits: creditedDeposits,
    pending_deposits: pendingDeposits.filter(
      (deposit) => !finalCreditedKeys.has(tapDepositKey(deposit))
    ),
    pending_reversals: pendingReversals.filter(
      (reversal) => !reversedKeys.has(tapDepositReversalKey(reversal))
    ),
    reversed_log_keys: Array.from(reversedKeys).sort(),
  };
  writeJson(cursorPath, nextCursor);

  const report = {
    ok: [...reversalResults, ...depositResults].every((item) => item.error === undefined),
    rpc_url_redacted: redactRpcUrl(rpc),
    pool: normalizeAddress(poolAddress, 'pool'),
    epoch,
    at: depositResults.at(-1)?.at
      ?? reversalResults.at(-1)?.at
      ?? explicitAt
      ?? Math.floor(Date.now() / 1000),
    tap_usd_au: tapUsdAu,
    confirmations,
    finalized_policy: scan.finalizedPolicy,
    finalized_reference_block: scan.referenceBlock,
    lookback_blocks: lookbackBlocks,
    from_block: scan.from,
    safe_to_block: scan.to,
    cursor: cursorPath,
    observed_deposit_count: scan.deposits.length,
    pending_deposit_count: nextCursor.pending_deposits.length,
    pending_reversal_count: nextCursor.pending_reversals.length,
    credited_deposit_count: nextCursor.credited_deposits.length,
    reversals: reversalResults,
    deposits: depositResults,
  };

  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('[tap:watch] TAP deposit watcher scan complete');
    console.log('[tap:watch] pool:', report.pool);
    console.log('[tap:watch] safe block:', scan.to);
    console.log('[tap:watch] pending reversals:', reversalResults.length);
    console.log('[tap:watch] matched deposits:', depositResults.length);
    for (const item of reversalResults) {
      console.log('Copy/paste admin TAP deposit reversal command:');
      console.log(item.copy_paste_admin_submit_command);
    }
    for (const item of depositResults) {
      console.log('Copy/paste admin TAP deposit submit command:');
      console.log(item.copy_paste_admin_submit_command);
    }
  }

  if (!report.ok) process.exit(2);
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(safeErrorMessage(error));
    process.exit(1);
  });
}
