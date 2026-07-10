#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { distribution } from './merkle.mjs';
import {
  POOL_SETTLEMENT_ABI,
  settlementDistributionEntries,
} from './tap-settlement-roller.mjs';

const scriptPath = fileURLToPath(import.meta.url);

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

function sameRoot(a, b) {
  if (!a || !b) return null;
  return String(a).toLowerCase() === String(b).toLowerCase();
}

async function maybePoolState(pool, account) {
  if (!pool) {
    return {
      chain_epoch: null,
      chain_root: null,
      claimed_wei: 0n,
      root_matches_chain: null,
    };
  }
  const [chainEpoch, chainRoot, claimedWei] = await Promise.all([
    pool.epoch(),
    pool.merkleRoot(),
    pool.claimed(account),
  ]);
  return {
    chain_epoch: Number(chainEpoch),
    chain_root: chainRoot,
    claimed_wei: claimedWei,
    root_matches_chain: null,
  };
}

function copyPasteClaimCall({ account, cumulativeWei, proof }) {
  const proofArgs = proof.map((item) => shellQuote(item)).join(' ');
  return `account=${shellQuote(account)} cumulative=${shellQuote(cumulativeWei)} proof=(${proofArgs})`;
}

export async function claimProofForAccount({
  settlement,
  account,
  pool = null,
} = {}) {
  const normalized = normalizeAddress(account, 'claim account');
  const entries = settlementDistributionEntries(settlement, 'settlement');
  const rootFromReport = settlement?.root ?? null;
  const epochFromReport = settlement?.epoch ?? null;
  const dist = entries.length > 0 ? distribution(entries) : null;
  const rebuiltRoot = dist?.root ?? null;
  const root = rootFromReport ?? rebuiltRoot;
  const rootMatchesReport = rootFromReport ? sameRoot(rebuiltRoot, rootFromReport) : true;
  const mine = entries.find((entry) => entry.account === normalized) ?? null;
  const chain = await maybePoolState(pool, normalized);
  const rootMatchesChain = chain.chain_root ? sameRoot(root, chain.chain_root) : null;

  if (!mine || !dist) {
    return {
      claimable: false,
      account: normalized,
      epoch: chain.chain_epoch ?? epochFromReport,
      root,
      chain_root: chain.chain_root,
      cumulative: null,
      cumulative_wei: null,
      claimed_wei: chain.claimed_wei.toString(),
      claimable_wei: '0',
      proof: null,
      leaf: null,
      root_matches_report: rootMatchesReport,
      root_matches_chain: rootMatchesChain,
      reason: 'account not in settlement distribution',
    };
  }

  const remainingWei = mine.amount > chain.claimed_wei ? mine.amount - chain.claimed_wei : 0n;
  const proof = dist.proofFor(normalized);
  const claimable = remainingWei > 0n && rootMatchesReport === true && rootMatchesChain !== false;
  let reason = null;
  if (!claimable) {
    if (remainingWei === 0n) reason = 'nothing to claim';
    else if (rootMatchesReport !== true) reason = 'report root does not match rebuilt distribution';
    else if (rootMatchesChain === false) reason = 'report root does not match live pool root';
  }

  return {
    claimable,
    account: normalized,
    epoch: chain.chain_epoch ?? epochFromReport,
    root,
    chain_root: chain.chain_root,
    cumulative: mine.amount.toString(),
    cumulative_wei: mine.amount.toString(),
    claimed_wei: chain.claimed_wei.toString(),
    claimable_wei: remainingWei.toString(),
    proof,
    leaf: dist.leafFor(normalized),
    root_matches_report: rootMatchesReport,
    root_matches_chain: rootMatchesChain,
    reason,
    copy_paste_claim_args: copyPasteClaimCall({
      account: normalized,
      cumulativeWei: mine.amount.toString(),
      proof,
    }),
  };
}

async function main() {
  const args = parseArgs();
  const settlementPath = args.settlement || args.report;
  if (!settlementPath) throw new Error('Missing --settlement/--report.');
  const account = args.account || args.provider || args.address;
  if (!account) throw new Error('Missing --account.');
  const settlement = readJson(path.resolve(settlementPath), 'settlement report');
  const ethRpc = args['eth-rpc'] || args.rpc || process.env.MAYHEM_TAP_ETH_RPC;
  const poolAddress = args.pool || process.env.MAYHEM_TAP_POOL_ADDRESS;
  let pool = null;
  if (ethRpc || poolAddress) {
    if (!ethRpc) throw new Error('Missing --eth-rpc or MAYHEM_TAP_ETH_RPC.');
    if (!poolAddress) throw new Error('Missing --pool or MAYHEM_TAP_POOL_ADDRESS.');
    const provider = new ethers.JsonRpcProvider(ethRpc);
    pool = new ethers.Contract(poolAddress, POOL_SETTLEMENT_ABI, provider);
  }
  const report = await claimProofForAccount({ settlement, account, pool });
  report.eth_rpc_configured = Boolean(ethRpc);
  report.copy_paste_replay_command = [
    'node',
    'contracts/scripts/tap-claim-proof.mjs',
    '--settlement',
    settlementPath,
    '--account',
    report.account,
    ...(poolAddress ? ['--pool', poolAddress] : []),
    '--json',
  ].map(shellQuote).join(' ');

  if (boolArg(args.json, false)) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('[tap:claim-proof] account:', report.account);
    console.log('[tap:claim-proof] epoch:', report.epoch ?? 'unknown');
    console.log('[tap:claim-proof] root:', report.root ?? 'none');
    console.log('[tap:claim-proof] cumulative_wei:', report.cumulative_wei ?? 'none');
    console.log('[tap:claim-proof] claimed_wei:', report.claimed_wei);
    console.log('[tap:claim-proof] claimable_wei:', report.claimable_wei);
    console.log('[tap:claim-proof] claimable:', report.claimable);
    if (report.reason) console.log('[tap:claim-proof] reason:', report.reason);
    console.log('Copy/paste claim() arguments:');
    console.log(report.copy_paste_claim_args ?? 'none');
    console.log('Copy/paste TAP claim-proof replay command:');
    console.log(report.copy_paste_replay_command);
  }
  if (!report.claimable && report.reason && report.reason !== 'nothing to claim') process.exitCode = 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}
