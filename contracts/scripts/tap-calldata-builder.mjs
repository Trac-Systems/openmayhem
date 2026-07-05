#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { ADDRESSES_FILE } from './deploy-local.mjs';
import { claimProofForAccount } from './tap-claim-proof.mjs';

const scriptPath = fileURLToPath(import.meta.url);

export const TAP_TOKEN_CALLDATA_ABI = [
  'function approve(address spender, uint256 amount) returns (bool)',
];

export const TAP_POOL_CALLDATA_ABI = [
  'function deposit(uint256 amount)',
  'function claim(address account, uint256 cumulativeAmount, bytes32[] proof)',
];

const tokenIface = new ethers.Interface(TAP_TOKEN_CALLDATA_ABI);
const poolIface = new ethers.Interface(TAP_POOL_CALLDATA_ABI);

function shellQuote(value) {
  const raw = String(value ?? '');
  if (raw.length === 0) return "''";
  return `'${raw.replaceAll("'", "'\\''")}'`;
}

function parseArgs(argv = process.argv.slice(2)) {
  const out = {};
  const positional = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) {
      positional.push(arg);
      continue;
    }
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) {
      out[key] = true;
    } else {
      out[key] = next;
      i += 1;
    }
  }
  out._ = positional;
  return out;
}

function boolArg(value, fallback = false) {
  if (value === undefined) return fallback;
  if (value === true) return true;
  const text = String(value).trim().toLowerCase();
  return ['1', 'true', 'yes', 'on'].includes(text);
}

function readJson(file, label) {
  const value = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (value === null || value === undefined) throw new Error(`${label} is empty`);
  return value;
}

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '')).toLowerCase();
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function parseNonNegativeInt(value, label) {
  if (value === undefined || value === null || value === '') return null;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) throw new Error(`${label} must be a non-negative safe integer`);
  return parsed;
}

function parseWei(value, label) {
  try {
    const parsed = BigInt(String(value ?? '').trim());
    if (parsed <= 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a positive integer wei amount`);
  }
}

export function tapAmountToWei(amountTap) {
  try {
    const wei = ethers.parseUnits(String(amountTap ?? ''), 18);
    if (wei <= 0n) throw new Error();
    return wei;
  } catch (_error) {
    throw new Error('amount TAP must be a positive decimal with at most 18 decimals');
  }
}

function readAddressesFile(file) {
  if (!file || !fs.existsSync(file)) return {};
  return readJson(file, 'TAP addresses file');
}

export function resolveTapAddresses({
  token,
  pool,
  chainId,
  addressesFile = ADDRESSES_FILE,
  env = process.env,
} = {}) {
  const fromFile = readAddressesFile(addressesFile);
  const resolvedPool = pool
    ?? env.MAYHEM_TAP_POOL_ADDRESS
    ?? env.TK_POOL_ADDR
    ?? fromFile.pool;
  const resolvedToken = token
    ?? env.MAYHEM_TAP_TOKEN_ADDRESS
    ?? env.TK_TOKEN_ADDR
    ?? fromFile.token;
  if (!resolvedPool) throw new Error('Missing TAP pool address');
  if (!resolvedToken) throw new Error('Missing TAP token address');
  const resolvedChainId = chainId
    ?? env.MAYHEM_TAP_CHAIN_ID
    ?? env.TK_ETH_CHAIN_ID
    ?? fromFile.chainId
    ?? null;
  return {
    pool: normalizeAddress(resolvedPool, 'TAP pool'),
    token: normalizeAddress(resolvedToken, 'TAP token'),
    chain_id: parseNonNegativeInt(resolvedChainId, 'chain id'),
  };
}

function txJson(tx) {
  return JSON.stringify(tx);
}

export function buildTapDepositCalldata({
  from,
  amountTap,
  amountWei,
  token,
  pool,
  chainId,
  addressesFile,
  env,
} = {}) {
  const addresses = resolveTapAddresses({ token, pool, chainId, addressesFile, env });
  const fromAddress = normalizeAddress(from, 'from');
  const wei = amountWei !== undefined && amountWei !== null && amountWei !== ''
    ? parseWei(amountWei, 'amount wei')
    : tapAmountToWei(amountTap);
  const approve = {
    to: addresses.token,
    data: tokenIface.encodeFunctionData('approve', [addresses.pool, wei]),
    value: '0x0',
  };
  const deposit = {
    to: addresses.pool,
    data: poolIface.encodeFunctionData('deposit', [wei]),
    value: '0x0',
  };
  return {
    rail: 'tap',
    kind: 'deposit',
    custody: 'external_wallet',
    server_signs: false,
    from: fromAddress,
    chain_id: addresses.chain_id,
    pool: addresses.pool,
    token: addresses.token,
    amount_tap: amountTap === undefined || amountTap === null ? null : String(amountTap),
    amount_wei: wei.toString(),
    transactions: [
      { step: 'approve', ...approve },
      { step: 'deposit', ...deposit },
    ],
    copy_paste: {
      approve_tx_json: txJson(approve),
      deposit_tx_json: txJson(deposit),
      order: 'Send approve first, wait for success, then send deposit from the same wallet.',
    },
  };
}

function parseProof(value) {
  if (Array.isArray(value)) return value.map(String);
  const text = String(value ?? '').trim();
  if (!text) throw new Error('proof is required');
  if (text.startsWith('[')) {
    const parsed = JSON.parse(text);
    if (!Array.isArray(parsed)) throw new Error('proof JSON must be an array');
    return parsed.map(String);
  }
  return text.split(/[,\s]+/).filter(Boolean);
}

export function buildTapClaimCalldata({
  account,
  cumulativeWei,
  proof,
  pool,
  token,
  chainId,
  addressesFile,
  env,
} = {}) {
  const addresses = resolveTapAddresses({ token, pool, chainId, addressesFile, env });
  const claimAccount = normalizeAddress(account, 'claim account');
  const cumulative = parseWei(cumulativeWei, 'cumulative wei');
  const proofItems = parseProof(proof);
  const transaction = {
    to: addresses.pool,
    data: poolIface.encodeFunctionData('claim', [claimAccount, cumulative, proofItems]),
    value: '0x0',
  };
  return {
    rail: 'tap',
    kind: 'claim',
    custody: 'external_wallet',
    server_signs: false,
    from: claimAccount,
    chain_id: addresses.chain_id,
    pool: addresses.pool,
    token: addresses.token,
    account: claimAccount,
    cumulative_wei: cumulative.toString(),
    proof: proofItems,
    transaction,
    copy_paste: {
      claim_tx_json: txJson(transaction),
    },
  };
}

async function claimFromSettlement(args) {
  if (!args.settlement && !args.report) return null;
  const settlement = readJson(path.resolve(args.settlement || args.report), 'settlement report');
  const account = args.account || args.provider || args.address;
  if (!account) throw new Error('Missing --account for claim settlement lookup');
  const proof = await claimProofForAccount({ settlement, account });
  if (!proof.proof || !proof.cumulative_wei) {
    throw new Error(proof.reason || 'claim proof not available');
  }
  if (!proof.claimable && !boolArg(args['allow-not-claimable'], false)) {
    throw new Error(proof.reason || 'claim is not currently claimable');
  }
  return proof;
}

function replayCommand(args, kind, report) {
  const out = ['node', 'contracts/scripts/tap-calldata-builder.mjs', kind];
  if (kind === 'deposit') {
    out.push('--from', report.from, '--amount-wei', report.amount_wei);
    out.push('--token', report.token, '--pool', report.pool);
  } else {
    out.push('--account', report.account, '--cumulative-wei', report.cumulative_wei);
    out.push('--proof', JSON.stringify(report.proof), '--pool', report.pool);
    out.push('--token', report.token);
  }
  if (report.chain_id !== null && report.chain_id !== undefined) out.push('--chain-id', String(report.chain_id));
  if (boolArg(args.json, false)) out.push('--json');
  return out.map(shellQuote).join(' ');
}

async function main() {
  const args = parseArgs();
  const kind = args.mode || args._[0] || (args.deposit ? 'deposit' : args.claim ? 'claim' : null);
  if (kind !== 'deposit' && kind !== 'claim') {
    throw new Error('Use mode/subcommand "deposit" or "claim".');
  }
  const common = {
    token: args.token,
    pool: args.pool,
    chainId: args['chain-id'],
    addressesFile: args.addresses ? path.resolve(args.addresses) : ADDRESSES_FILE,
  };
  let report;
  if (kind === 'deposit') {
    report = buildTapDepositCalldata({
      ...common,
      from: args.from || args.account || args.address,
      amountTap: args['amount-tap'] ?? args.tap ?? args.amount,
      amountWei: args['amount-wei'],
    });
  } else {
    const proof = args['claim-proof']
      ? readJson(path.resolve(args['claim-proof']), 'claim proof')
      : await claimFromSettlement(args);
    report = buildTapClaimCalldata({
      ...common,
      account: args.account || args.provider || args.address || proof?.account,
      cumulativeWei: args['cumulative-wei'] ?? args.cumulative ?? proof?.cumulative_wei ?? proof?.cumulative,
      proof: args.proof ?? proof?.proof,
    });
  }
  report.copy_paste_replay_command = replayCommand(args, kind, report);

  if (boolArg(args.json, false)) {
    console.log(JSON.stringify(report, null, 2));
  } else if (kind === 'deposit') {
    console.log('[tap:calldata] deposit amount_wei:', report.amount_wei);
    console.log('[tap:calldata] from:', report.from);
    console.log('[tap:calldata] token:', report.token);
    console.log('[tap:calldata] pool:', report.pool);
    console.log('Copy/paste approve tx JSON:');
    console.log(report.copy_paste.approve_tx_json);
    console.log('Copy/paste deposit tx JSON:');
    console.log(report.copy_paste.deposit_tx_json);
    console.log('Copy/paste TAP calldata replay command:');
    console.log(report.copy_paste_replay_command);
  } else {
    console.log('[tap:calldata] claim account:', report.account);
    console.log('[tap:calldata] cumulative_wei:', report.cumulative_wei);
    console.log('[tap:calldata] pool:', report.pool);
    console.log('Copy/paste claim tx JSON:');
    console.log(report.copy_paste.claim_tx_json);
    console.log('Copy/paste TAP calldata replay command:');
    console.log(report.copy_paste_replay_command);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}
