#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

const scriptPath = fileURLToPath(import.meta.url);
const DEFAULT_CURSOR = path.resolve('.mayhem-local', 'tap-deposit-watcher.json');
const TAP_WEI = 1_000_000_000_000_000_000n;
const MIN_TAP_CONFIRMATIONS = 12;
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
  return `${deposit.eth_tx_hash}/${deposit.log_index}`;
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
  return seen !== null
    && seen?.eth_tx_hash === deposit.eth_tx_hash
    && Number(seen?.log_index) === deposit.log_index
    && seen?.who === deposit.who
    && seen?.tap_wei === deposit.tap_wei
    && seen?.block_hash === deposit.block_hash
    && Number(seen?.finalized_block_number) === Number(deposit.finalized_block_number)
    && Number(seen?.confirmation_depth) === Number(deposit.confirmation_depth)
    && seen?.confirmation_policy === deposit.confirmation_policy
    && seen?.event_signature === deposit.event_signature
    && seen?.watcher_id === deposit.watcher_id
    && (!hasExpectedAu || readAu(seen?.au) === expectedAu)
    && balance?.user === deposit.who
    && balance?.rail === 'tap'
    && balance?.denom === 'au_usd'
    && balanceAu !== null
    && (!hasExpectedAu || balanceAu >= expectedAu)
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
  timeoutMs = 30_000,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = Date.now() + timeoutMs;
  const seenKey = `dep/tap/${tapDepositKey(deposit)}`;
  let state = null;
  while (Date.now() <= deadline) {
    const [seen, balance, depositRoot] = await Promise.all([
      readContractStateValue(rpcUrl, seenKey, { fetchImpl }),
      readContractStateValue(rpcUrl, `bal/${deposit.who}/tap`, { fetchImpl }),
      readContractStateValue(rpcUrl, `ev/dep/${epoch}`, { fetchImpl }),
    ]);
    state = { seen, balance, depositRoot };
    if (tapDepositStateMatches(deposit, { ...state, epoch })) {
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

function normalizeCursor(raw) {
  const cursor = raw && typeof raw === 'object' ? raw : {};
  return {
    next_block: Number.isSafeInteger(cursor.next_block) && cursor.next_block >= 0
      ? cursor.next_block
      : 0,
    seen_log_keys: Array.isArray(cursor.seen_log_keys)
      ? cursor.seen_log_keys.filter((key) => typeof key === 'string')
      : [],
    unsubmitted_deposits: Array.isArray(cursor.unsubmitted_deposits)
      ? cursor.unsubmitted_deposits.filter((deposit) => deposit?.eth_tx_hash && Number.isSafeInteger(deposit?.log_index))
      : [],
  };
}

async function safeToBlock(provider, { toBlock, confirmations = 12, blockTag }) {
  if (blockTag === 'finalized') {
    const block = await provider.send('eth_getBlockByNumber', ['finalized', false]);
    if (!block || block.number == null) throw new Error('finalized block tag unavailable from RPC');
    const safeTo = Number(block.number);
    return { safeTo, referenceBlock: safeTo, finalizedPolicy: true };
  }
  const head = Number(await provider.send('eth_blockNumber', []));
  const depth = parseNonNegativeInt(confirmations, '--confirmations', 12);
  if (depth < MIN_TAP_CONFIRMATIONS) {
    throw new Error(`--confirmations must be at least ${MIN_TAP_CONFIRMATIONS} unless --block-tag finalized is used`);
  }
  const maxSafe = Math.max(-1, head - depth);
  if (toBlock !== undefined && toBlock !== null && toBlock !== '') {
    const explicit = parseNonNegativeInt(toBlock, '--to-block');
    if (explicit > maxSafe) {
      throw new Error(`--to-block must be at least ${MIN_TAP_CONFIRMATIONS} confirmations deep unless --block-tag finalized is used`);
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
  tapUsdAu,
  chainId,
  poolAddress,
} = {}) {
  const provider = pool.runner?.provider ?? pool.runner ?? pool.provider;
  if (!provider) throw new Error('pool contract has no provider');
  const network = chainId ? null : await provider.getNetwork();
  const normalizedChainId = Number(chainId ?? network.chainId);
  const { safeTo, referenceBlock, finalizedPolicy } = await safeToBlock(provider, { toBlock, confirmations, blockTag });
  const from = parseNonNegativeInt(fromBlock, '--from-block', 0);
  if (safeTo < from) return { deposits: [], from, to: from - 1 };

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
  return { deposits, from, to: safeTo };
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
  const at = parseNonNegativeInt(args.at ?? Math.floor(Date.now() / 1000), '--at');
  const cursorPath = path.resolve(args.cursor || DEFAULT_CURSOR);
  const cursor = normalizeCursor(readJsonIfExists(cursorPath, {}));
  const fromBlock = args['from-block'] !== undefined
    ? parseNonNegativeInt(args['from-block'], '--from-block')
    : cursor.next_block;
  const confirmations = parseNonNegativeInt(args.confirmations ?? 12, '--confirmations', 12);
  const chunkSize = parsePositiveInt(args['chunk-size'] ?? 5000, '--chunk-size', 5000);
  const submit = boolArg(args.submit, false);
  const sim = boolArg(args.sim, false);
  const json = boolArg(args.json, false);
  const verify = !boolArg(args['no-verify'], false);
  const verifyTimeoutMs = parseNonNegativeInt(args['verify-timeout-ms'] ?? 30_000, '--verify-timeout-ms', 30_000);
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
    tapUsdAu,
    chainId: args['chain-id'] ? parsePositiveInt(args['chain-id'], '--chain-id') : undefined,
    poolAddress,
  });

  const seen = new Set(cursor.seen_log_keys);
  const unsubmitted = mergeDeposits(cursor.unsubmitted_deposits, scan.deposits)
    .filter((deposit) => !seen.has(tapDepositKey(deposit)));
  const adminOptions = {
    epoch,
    at,
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

  const confirmationsOut = [];
  const submittedKeys = new Set();
  for (const deposit of unsubmitted) {
    const command = buildAdminCommand(deposit, adminOptions);
    const confirmation = {
      ...deposit,
      copy_paste_admin_submit_command: command,
      submitted: false,
      preflighted: false,
      verified: false,
    };
    if (submit) {
      const child = spawnSync(
        args['mayhem-bin'] || 'mayhem',
        buildAdminCommandArgs(deposit, adminOptions),
        { encoding: 'utf8' }
      );
      if (sim) {
        confirmation.preflighted = child.status === 0;
      } else {
        confirmation.submitted = child.status === 0;
      }
      confirmation.exit_status = child.status;
      confirmation.stdout = child.stdout?.trim() || null;
      confirmation.stderr = child.stderr?.trim() || null;
      if (child.status !== 0) {
        confirmation.error = `mayhem admin tap-deposit exited ${child.status}`;
      } else if (!sim) {
        if (adminOptions.rpcUrl && verify) {
          const verification = await waitForTapDepositState(deposit, {
            rpcUrl: adminOptions.rpcUrl,
            epoch,
            timeoutMs: verifyTimeoutMs,
            pollMs: verifyPollMs,
          });
          confirmation.verified = verification.verified;
          confirmation.state = verification.state;
          if (verification.verified) {
            submittedKeys.add(tapDepositKey(deposit));
          } else {
            confirmation.error = 'tapDeposit submit did not update seen log, balance, and deposit root';
          }
        } else {
          confirmation.verification_skipped = true;
          submittedKeys.add(tapDepositKey(deposit));
        }
      }
    }
    confirmationsOut.push(confirmation);
  }

  for (const key of submittedKeys) seen.add(key);
  const nextCursor = {
    next_block: Math.max(cursor.next_block, scan.to + 1),
    seen_log_keys: Array.from(seen).sort(),
    unsubmitted_deposits: unsubmitted.filter((deposit) => !seen.has(tapDepositKey(deposit))),
  };
  writeJson(cursorPath, nextCursor);

  const report = {
    ok: confirmationsOut.every((item) => item.error === undefined),
    rpc,
    pool: normalizeAddress(poolAddress, 'pool'),
    epoch,
    at,
    tap_usd_au: tapUsdAu,
    confirmations,
    from_block: scan.from,
    safe_to_block: scan.to,
    cursor: cursorPath,
    observed_deposit_count: scan.deposits.length,
    unsubmitted_deposit_count: nextCursor.unsubmitted_deposits.length,
    confirmations: confirmationsOut,
  };

  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('[tap:watch] TAP deposit watcher scan complete');
    console.log('[tap:watch] pool:', report.pool);
    console.log('[tap:watch] safe block:', scan.to);
    console.log('[tap:watch] matched deposits:', confirmationsOut.length);
    for (const item of confirmationsOut) {
      console.log('Copy/paste admin TAP deposit submit command:');
      console.log(item.copy_paste_admin_submit_command);
    }
  }

  if (!report.ok) process.exit(2);
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}
