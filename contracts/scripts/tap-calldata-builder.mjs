#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { ADDRESSES_FILE } from './paths.mjs';
import { claimProofForAccount } from './tap-claim-proof.mjs';

const scriptPath = fileURLToPath(import.meta.url);

export const TAP_TOKEN_CALLDATA_ABI = [
  'function approve(address spender, uint256 amount) returns (bool)',
  'function allowance(address owner, address spender) view returns (uint256)',
  'function balanceOf(address account) view returns (uint256)',
];

export const TAP_POOL_CALLDATA_ABI = [
  'function deposit(uint256 amount)',
  'function claim(address account, uint256 cumulativeAmount, bytes32[] proof)',
  'function claimed(address account) view returns (uint256)',
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
    ?? fromFile.pool;
  const resolvedToken = token
    ?? env.MAYHEM_TAP_TOKEN_ADDRESS
    ?? env.MAYHEM_TAP_TOKEN_ADDR
    ?? fromFile.token;
  if (!resolvedPool) throw new Error('Missing TAP pool address');
  if (!resolvedToken) throw new Error('Missing TAP token address');
  const resolvedChainId = chainId
    ?? env.MAYHEM_TAP_ETH_CHAIN_ID
    ?? env.MAYHEM_TAP_CHAIN_ID
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

function bigintString(value) {
  return value === null || value === undefined ? null : BigInt(value).toString();
}

function weiToEthDecimal(wei) {
  return ethers.formatEther(BigInt(wei));
}

function normalizeRpcUrl(value, env = process.env) {
  const rpc = String(value ?? env.MAYHEM_TAP_ETH_RPC ?? 'https://ethereum-rpc.publicnode.com').trim();
  if (!rpc) throw new Error('Missing Ethereum RPC URL');
  return rpc;
}

function normalizePrivateKey(value, label = 'Ethereum wallet private key') {
  const key = String(value ?? '').trim();
  if (!/^0x[0-9a-fA-F]{64}$/.test(key)) {
    throw new Error(`${label} must be a 0x-prefixed 32-byte private key`);
  }
  return key;
}

async function feePriceWei(provider) {
  const fee = await provider.getFeeData();
  return fee.maxFeePerGas ?? fee.gasPrice ?? 0n;
}

function bufferedGasCost(gas, priceWei, bufferBps = 12_000n) {
  return (BigInt(gas) * BigInt(priceWei) * BigInt(bufferBps)) / 10_000n;
}

async function simulateContractStep(call, args = []) {
  const out = {
    static_call_ok: false,
    gas_estimate_ok: false,
    gas_estimate: null,
    error: null,
  };
  try {
    if (call.staticCall) {
      await call.staticCall(...args);
    } else {
      await call(...args);
    }
    out.static_call_ok = true;
  } catch (error) {
    out.error = error?.shortMessage || error?.reason || error?.message || String(error);
  }
  try {
    if (call.estimateGas) {
      out.gas_estimate = (await call.estimateGas(...args)).toString();
      out.gas_estimate_ok = true;
    }
  } catch (error) {
    out.error = out.error || error?.shortMessage || error?.reason || error?.message || String(error);
  }
  return out;
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

export async function executeTapDeposit({
  privateKey,
  ethRpc,
  amountTap,
  amountWei,
  token,
  pool,
  chainId,
  addressesFile,
  confirm = false,
  provider: injectedProvider = null,
  env = process.env,
} = {}) {
  const addresses = resolveTapAddresses({ token, pool, chainId, addressesFile, env });
  const wei = amountWei !== undefined && amountWei !== null && amountWei !== ''
    ? parseWei(amountWei, 'amount wei')
    : tapAmountToWei(amountTap);
  const rpc = injectedProvider ? null : normalizeRpcUrl(ethRpc, env);
  const provider = injectedProvider ?? new ethers.JsonRpcProvider(rpc);
  const signer = new ethers.NonceManager(new ethers.Wallet(normalizePrivateKey(privateKey), provider));
  const from = await signer.getAddress();
  const normalizedFrom = normalizeAddress(from, 'wallet address');
  const tokenContract = new ethers.Contract(addresses.token, TAP_TOKEN_CALLDATA_ABI, signer);
  const poolContract = new ethers.Contract(addresses.pool, TAP_POOL_CALLDATA_ABI, signer);
  const network = await provider.getNetwork();
  const chain = Number(network.chainId);
  if (addresses.chain_id !== null && addresses.chain_id !== undefined && chain !== addresses.chain_id) {
    throw new Error(`Ethereum RPC chain id ${chain} does not match TAP chain id ${addresses.chain_id}`);
  }

  const [ethBalance, tapBalance, allowance, gasPriceWei] = await Promise.all([
    provider.getBalance(normalizedFrom),
    tokenContract.balanceOf(normalizedFrom),
    tokenContract.allowance(normalizedFrom, addresses.pool),
    feePriceWei(provider),
  ]);
  if (tapBalance < wei) {
    throw new Error(
      `not enough TAP - send ${ethers.formatUnits(wei - tapBalance, 18)} TAP to ${from} before depositing`
    );
  }

  const approvalNeeded = allowance < wei;
  const approve = approvalNeeded
    ? await simulateContractStep(tokenContract.approve, [addresses.pool, wei])
    : {
        static_call_ok: true,
        gas_estimate_ok: true,
        gas_estimate: '0',
        error: null,
        skipped: true,
        reason: 'allowance already covers deposit',
      };
  if (!approve.static_call_ok || !approve.gas_estimate_ok) {
    throw new Error(`TAP approve simulation failed: ${approve.error || 'unknown error'}`);
  }

  let deposit = null;
  if (!approvalNeeded) {
    deposit = await simulateContractStep(poolContract.deposit, [wei]);
    if (!deposit.static_call_ok || !deposit.gas_estimate_ok) {
      throw new Error(`TAP deposit simulation failed: ${deposit.error || 'unknown error'}`);
    }
  }

  const approveGas = BigInt(approve.gas_estimate ?? '0');
  const depositGasForPrecheck = deposit?.gas_estimate ? BigInt(deposit.gas_estimate) : 200_000n;
  const estimatedTotalGas = approveGas + depositGasForPrecheck;
  const estimatedMaxCostWei = bufferedGasCost(estimatedTotalGas, gasPriceWei);
  const precheck = {
    ok: ethBalance >= estimatedMaxCostWei,
    eth_balance_wei: ethBalance.toString(),
    gas_price_wei: gasPriceWei.toString(),
    estimated_total_gas: estimatedTotalGas.toString(),
    estimated_max_cost_wei: estimatedMaxCostWei.toString(),
    estimated_max_cost_eth: weiToEthDecimal(estimatedMaxCostWei),
    shortfall_wei: ethBalance >= estimatedMaxCostWei ? '0' : (estimatedMaxCostWei - ethBalance).toString(),
    shortfall_eth: ethBalance >= estimatedMaxCostWei ? '0.0' : weiToEthDecimal(estimatedMaxCostWei - ethBalance),
  };
  if (!precheck.ok) {
    throw new Error(
      `not enough ETH for gas - send about ${precheck.shortfall_eth} ETH to ${from} before depositing`
    );
  }

  const report = {
    rail: 'tap',
    kind: 'deposit',
    custody: 'local_wallet',
    server_signs: false,
    submitted: false,
    confirm: Boolean(confirm),
    eth_rpc_configured: true,
    from,
    chain_id: chain,
    pool: addresses.pool,
    token: addresses.token,
    amount_tap: amountTap === undefined || amountTap === null ? null : String(amountTap),
    amount_wei: wei.toString(),
    balances: {
      eth_wei: ethBalance.toString(),
      tap_wei: tapBalance.toString(),
      allowance_wei: allowance.toString(),
    },
    gas_precheck: precheck,
    simulation: {
      approve,
      deposit: deposit ?? {
        static_call_ok: false,
        gas_estimate_ok: false,
        gas_estimate: null,
        deferred_until_after_approval: true,
        reason: 'current allowance is below deposit amount; deposit is simulated after approve confirms and before value moves',
      },
    },
    transactions: [],
  };

  if (!confirm) {
    return report;
  }

  if (approvalNeeded) {
    const tx = await tokenContract.approve(addresses.pool, wei);
    const receipt = await tx.wait();
    report.transactions.push({
      step: 'approve',
      tx_hash: tx.hash,
      status: Number(receipt?.status ?? 0),
      gas_used: bigintString(receipt?.gasUsed),
      block_number: receipt?.blockNumber ?? null,
    });
    if (receipt?.status !== 1) throw new Error(`TAP approve tx failed: ${tx.hash}`);
  }

  const depositAfterApproval = await simulateContractStep(poolContract.deposit, [wei]);
  report.simulation.deposit = depositAfterApproval;
  if (!depositAfterApproval.static_call_ok || !depositAfterApproval.gas_estimate_ok) {
    throw new Error(`TAP deposit simulation failed after approve: ${depositAfterApproval.error || 'unknown error'}`);
  }
  const latestEthBalance = await provider.getBalance(normalizedFrom);
  const depositCost = bufferedGasCost(BigInt(depositAfterApproval.gas_estimate), gasPriceWei);
  if (latestEthBalance < depositCost) {
    throw new Error(
      `not enough ETH for deposit gas after approve - send about ${weiToEthDecimal(depositCost - latestEthBalance)} ETH to ${from}`
    );
  }

  const tx = await poolContract.deposit(wei);
  const receipt = await tx.wait();
  report.transactions.push({
    step: 'deposit',
    tx_hash: tx.hash,
    status: Number(receipt?.status ?? 0),
    gas_used: bigintString(receipt?.gasUsed),
    block_number: receipt?.blockNumber ?? null,
  });
  if (receipt?.status !== 1) throw new Error(`TAP deposit tx failed: ${tx.hash}`);
  report.submitted = true;
  report.deposit_tx_hash = tx.hash;
  return report;
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

export async function executeTapClaim({
  privateKey,
  ethRpc,
  account,
  cumulativeWei,
  proof,
  pool,
  token,
  chainId,
  addressesFile,
  confirm = false,
  provider: injectedProvider = null,
  env = process.env,
} = {}) {
  const addresses = resolveTapAddresses({ token, pool, chainId, addressesFile, env });
  const claimAccount = normalizeAddress(account, 'claim account');
  const cumulative = parseWei(cumulativeWei, 'cumulative wei');
  const proofItems = parseProof(proof);
  const rpc = injectedProvider ? null : normalizeRpcUrl(ethRpc, env);
  const provider = injectedProvider ?? new ethers.JsonRpcProvider(rpc);
  const signer = new ethers.NonceManager(new ethers.Wallet(normalizePrivateKey(privateKey), provider));
  const from = await signer.getAddress();
  if (normalizeAddress(from, 'wallet address') !== claimAccount) {
    throw new Error(`local Ethereum wallet ${from} does not match claim account ${claimAccount}`);
  }

  const network = await provider.getNetwork();
  const chain = Number(network.chainId);
  if (addresses.chain_id !== null && addresses.chain_id !== undefined && chain !== addresses.chain_id) {
    throw new Error(`Ethereum RPC chain id ${chain} does not match TAP chain id ${addresses.chain_id}`);
  }

  const poolContract = new ethers.Contract(addresses.pool, TAP_POOL_CALLDATA_ABI, signer);
  const tokenContract = new ethers.Contract(addresses.token, TAP_TOKEN_CALLDATA_ABI, provider);
  const [ethBalance, tapBalanceBefore, alreadyClaimed, gasPriceWei] = await Promise.all([
    provider.getBalance(claimAccount),
    tokenContract.balanceOf(claimAccount),
    poolContract.claimed(claimAccount),
    feePriceWei(provider),
  ]);
  if (cumulative <= alreadyClaimed) {
    throw new Error('nothing to claim');
  }

  const simulation = await simulateContractStep(poolContract.claim, [
    claimAccount,
    cumulative,
    proofItems,
  ]);
  if (!simulation.static_call_ok || !simulation.gas_estimate_ok) {
    throw new Error(`TAP claim simulation failed: ${simulation.error || 'unknown error'}`);
  }
  const estimatedMaxCostWei = bufferedGasCost(BigInt(simulation.gas_estimate), gasPriceWei);
  const gasPrecheck = {
    ok: ethBalance >= estimatedMaxCostWei,
    eth_balance_wei: ethBalance.toString(),
    gas_price_wei: gasPriceWei.toString(),
    estimated_gas: simulation.gas_estimate,
    estimated_max_cost_wei: estimatedMaxCostWei.toString(),
    estimated_max_cost_eth: weiToEthDecimal(estimatedMaxCostWei),
    shortfall_wei: ethBalance >= estimatedMaxCostWei ? '0' : (estimatedMaxCostWei - ethBalance).toString(),
    shortfall_eth: ethBalance >= estimatedMaxCostWei ? '0.0' : weiToEthDecimal(estimatedMaxCostWei - ethBalance),
  };
  if (!gasPrecheck.ok) {
    throw new Error(
      `not enough ETH for claim gas - send about ${gasPrecheck.shortfall_eth} ETH to ${from}`
    );
  }

  const report = {
    rail: 'tap',
    kind: 'claim',
    custody: 'local_wallet',
    server_signs: false,
    submitted: false,
    confirm: Boolean(confirm),
    eth_rpc_configured: true,
    from,
    chain_id: chain,
    pool: addresses.pool,
    token: addresses.token,
    account: claimAccount,
    cumulative_wei: cumulative.toString(),
    previously_claimed_wei: alreadyClaimed.toString(),
    claimable_wei: (cumulative - alreadyClaimed).toString(),
    proof: proofItems,
    balances: {
      eth_wei: ethBalance.toString(),
      tap_wei_before: tapBalanceBefore.toString(),
    },
    gas_precheck: gasPrecheck,
    simulation,
    transaction: null,
  };
  if (!confirm) return report;

  const tx = await poolContract.claim(claimAccount, cumulative, proofItems);
  const receipt = await tx.wait();
  report.transaction = {
    step: 'claim',
    tx_hash: tx.hash,
    status: Number(receipt?.status ?? 0),
    gas_used: bigintString(receipt?.gasUsed),
    block_number: receipt?.blockNumber ?? null,
  };
  if (receipt?.status !== 1) throw new Error(`TAP claim tx failed: ${tx.hash}`);
  const [tapBalanceAfter, claimedAfter] = await Promise.all([
    tokenContract.balanceOf(claimAccount),
    poolContract.claimed(claimAccount),
  ]);
  report.submitted = true;
  report.claim_tx_hash = tx.hash;
  report.claimed_wei = claimedAfter.toString();
  report.balances.tap_wei_after = tapBalanceAfter.toString();
  report.balances.tap_delta_wei = (tapBalanceAfter - tapBalanceBefore).toString();
  return report;
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
    if (boolArg(args['local-wallet'], false) || boolArg(args.execute, false) || boolArg(args.confirm, false)) {
      report = await executeTapDeposit({
        ...common,
        privateKey: args['private-key'] || process.env.MAYHEM_TAP_WALLET_PRIVATE_KEY,
        ethRpc: args['eth-rpc'] || args['rpc-url'] || args.rpc,
        amountTap: args['amount-tap'] ?? args.tap ?? args.amount,
        amountWei: args['amount-wei'],
        confirm: boolArg(args.confirm, false),
      });
    } else {
      report = buildTapDepositCalldata({
        ...common,
        from: args.from || args.account || args.address,
        amountTap: args['amount-tap'] ?? args.tap ?? args.amount,
        amountWei: args['amount-wei'],
      });
    }
  } else {
    const proof = args['claim-proof']
      ? readJson(path.resolve(args['claim-proof']), 'claim proof')
      : await claimFromSettlement(args);
    const claim = {
      ...common,
      account: args.account || args.provider || args.address || proof?.account,
      cumulativeWei: args['cumulative-wei'] ?? args.cumulative ?? proof?.cumulative_wei ?? proof?.cumulative,
      proof: args.proof ?? proof?.proof,
    };
    if (boolArg(args['local-wallet'], false) || boolArg(args.execute, false) || boolArg(args.confirm, false)) {
      report = await executeTapClaim({
        ...claim,
        privateKey: args['private-key'] || process.env.MAYHEM_TAP_WALLET_PRIVATE_KEY,
        ethRpc: args['eth-rpc'] || args['rpc-url'] || args.rpc,
        confirm: boolArg(args.confirm, false),
      });
    } else {
      report = buildTapClaimCalldata(claim);
    }
  }
  if (report.custody !== 'local_wallet') {
    report.copy_paste_replay_command = replayCommand(args, kind, report);
  }

  if (boolArg(args.json, false)) {
    console.log(JSON.stringify(report, null, 2));
  } else if (kind === 'deposit' && report.custody === 'local_wallet') {
    console.log(report.submitted ? '[tap:deposit] submitted' : '[tap:deposit] dry-run');
    console.log('[tap:deposit] from:', report.from);
    console.log('[tap:deposit] token:', report.token);
    console.log('[tap:deposit] pool:', report.pool);
    console.log('[tap:deposit] amount_wei:', report.amount_wei);
    console.log('[tap:deposit] gas_precheck:', report.gas_precheck?.ok ? 'ok' : 'failed');
    console.log('[tap:deposit] approve gas:', report.simulation?.approve?.gas_estimate ?? '');
    console.log('[tap:deposit] deposit gas:', report.simulation?.deposit?.gas_estimate ?? '(after approve)');
    if (report.transactions?.length) {
      for (const tx of report.transactions) console.log(`[tap:deposit] ${tx.step}:`, tx.tx_hash);
    } else {
      console.log('Dry run only. Re-run with --confirm to broadcast after reviewing the simulation.');
    }
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
  } else if (report.custody === 'local_wallet') {
    console.log(report.submitted ? '[tap:claim] submitted' : '[tap:claim] dry-run');
    console.log('[tap:claim] account:', report.account);
    console.log('[tap:claim] cumulative_wei:', report.cumulative_wei);
    console.log('[tap:claim] claimable_wei:', report.claimable_wei);
    console.log('[tap:claim] pool:', report.pool);
    console.log('[tap:claim] gas_precheck:', report.gas_precheck?.ok ? 'ok' : 'failed');
    console.log('[tap:claim] claim gas:', report.simulation?.gas_estimate ?? '');
    if (report.transaction) {
      console.log('[tap:claim] claim:', report.transaction.tx_hash);
    } else {
      console.log('Dry run only. Re-run with --confirm to broadcast after reviewing the simulation.');
    }
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
