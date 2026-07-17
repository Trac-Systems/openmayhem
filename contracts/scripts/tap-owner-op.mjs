#!/usr/bin/env node
// Cross-signed, delayed MayhemInferencePool governance operations. Dry-run by default.
import fs from 'node:fs';

import { ethers } from 'ethers';

import { DEFAULT_MAINNET_DEPLOYMENT_FILE } from './deploy-mainnet.mjs';
import {
  signMaxEpochDeltaProposal,
  signRescueProposal,
} from './pool-governance.mjs';
import { safeErrorMessage } from './safe-output.mjs';

export const POOL_OWNER_ABI = [
  'function owner() view returns (address)',
  'function governanceSigner() view returns (address)',
  'function maxEpochDelta() view returns (uint256)',
  'function governanceNonce() view returns (uint256)',
  'function proposeMaxEpochDelta(uint256 newMax, bytes governanceSignature)',
  'function executeMaxEpochDelta()',
  'function pendingMaxEpochDelta() view returns (uint256 newMaxEpochDelta, uint256 nonce, uint64 executeAfter)',
  'function rescuableSurplus() view returns (uint256)',
  'function proposeRescue(address to, uint256 amount, bytes governanceSignature)',
  'function executeRescue()',
  'function pendingRescue() view returns (address to, uint256 amount, uint256 nonce, uint64 executeAfter)',
];

function parseArgs(argv = process.argv.slice(2)) {
  const out = {};
  const positional = [];
  for (let i = 0; i < argv.length; i += 1) {
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
  return ['1', 'true', 'yes', 'on'].includes(String(value).trim().toLowerCase());
}

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '').trim());
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function parseNonNegativeBigInt(value, label) {
  try {
    const parsed = BigInt(String(value ?? '').trim());
    if (parsed < 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a non-negative integer wei amount`);
  }
}

function readJsonIfExists(file) {
  if (!file || !fs.existsSync(file)) return {};
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function resolvePoolAddress(args, env = process.env) {
  const deploymentFile = args['deployment-file']
    ?? env.MAYHEM_TAP_DEPLOYMENT_FILE
    ?? DEFAULT_MAINNET_DEPLOYMENT_FILE;
  const deployment = readJsonIfExists(deploymentFile);
  return normalizeAddress(args.pool ?? env.MAYHEM_TAP_POOL_ADDRESS ?? deployment.pool, 'pool');
}

function privateKeyFromFile(file) {
  const parsed = readJsonIfExists(file);
  return parsed.privateKey ?? parsed.private_key ?? null;
}

function resolvePrivateKey({ envValue, file, label }) {
  const key = envValue ?? privateKeyFromFile(file);
  if (!/^0x[0-9a-fA-F]{64}$/.test(String(key ?? '').trim())) {
    throw new Error(`Missing ${label}; configure its environment variable or key file.`);
  }
  return String(key).trim();
}

function ownerWallet(args, env, provider) {
  const key = resolvePrivateKey({
    envValue: env.MAYHEM_TAP_POOL_OWNER_PRIVATE_KEY,
    file: args['owner-key-file'] ?? env.MAYHEM_TAP_POOL_OWNER_KEY_FILE,
    label: 'TAP pool owner private key',
  });
  return new ethers.Wallet(key, provider);
}

function governanceWallet(args, env, provider) {
  const key = resolvePrivateKey({
    envValue: env.MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY,
    file: args['governance-key-file'] ?? env.MAYHEM_TAP_GOVERNANCE_KEY_FILE,
    label: 'independent TAP governance private key',
  });
  return new ethers.Wallet(key, provider);
}

function errorMessage(error) {
  return safeErrorMessage(error);
}

async function simulate(method, params = []) {
  const report = { ok: false, static_call_ok: false, gas_estimate_ok: false, gas_estimate: null, error: null };
  try {
    await method.staticCall(...params);
    report.static_call_ok = true;
  } catch (error) {
    report.error = errorMessage(error);
  }
  try {
    report.gas_estimate = (await method.estimateGas(...params)).toString();
    report.gas_estimate_ok = true;
  } catch (error) {
    report.error = report.error || errorMessage(error);
  }
  report.ok = report.static_call_ok && report.gas_estimate_ok;
  return report;
}

function pendingMaxRecord(value) {
  return {
    new_max_epoch_delta_wei: BigInt(value.newMaxEpochDelta ?? value[0]).toString(),
    nonce: BigInt(value.nonce ?? value[1]).toString(),
    execute_after: Number(value.executeAfter ?? value[2]),
  };
}

function pendingRescueRecord(value) {
  return {
    to: normalizeAddress(value.to ?? value[0], 'pending rescue destination'),
    amount_wei: BigInt(value.amount ?? value[1]).toString(),
    nonce: BigInt(value.nonce ?? value[2]).toString(),
    execute_after: Number(value.executeAfter ?? value[3]),
  };
}

export async function buildOwnerOpPlan({ args = parseArgs(), env = process.env } = {}) {
  const command = String(args._?.[0] || args.command || '');
  const supported = new Set([
    'propose-max-epoch-delta',
    'execute-max-epoch-delta',
    'propose-rescue',
    'execute-rescue',
  ]);
  if (!supported.has(command)) throw new Error(`Unsupported owner op: ${command || '<missing>'}`);
  const ethRpc = String(args['eth-rpc'] ?? args.rpc ?? env.MAYHEM_TAP_ETH_RPC ?? '').trim();
  if (!ethRpc) throw new Error('Missing Ethereum RPC URL');
  const provider = new ethers.JsonRpcProvider(ethRpc);
  try {
    const poolAddress = resolvePoolAddress(args, env);
    const owner = ownerWallet(args, env, provider);
    const pool = new ethers.Contract(poolAddress, POOL_OWNER_ABI, owner);
    const [network, contractOwner, contractGovernanceSigner, currentMax, nonce] = await Promise.all([
      provider.getNetwork(),
      pool.owner(),
      pool.governanceSigner(),
      pool.maxEpochDelta(),
      pool.governanceNonce(),
    ]);
    const signingAddress = normalizeAddress(await owner.getAddress(), 'owner signing address');
    const report = {
      rail: 'tap',
      command,
      submitted: false,
      chain_id: Number(network.chainId),
      pool: poolAddress,
      owner: normalizeAddress(contractOwner, 'owner()'),
      signing_address: signingAddress,
      signer_is_owner: signingAddress.toLowerCase() === String(contractOwner).toLowerCase(),
      governance_signer: normalizeAddress(contractGovernanceSigner, 'governanceSigner()'),
      governance_nonce: nonce.toString(),
      current_max_epoch_delta_wei: currentMax.toString(),
    };

    let method;
    let params = [];
    if (command === 'propose-max-epoch-delta') {
      const next = parseNonNegativeBigInt(
        args['max-epoch-delta'] ?? args.max ?? env.MAYHEM_TAP_MAX_EPOCH_DELTA,
        'max epoch delta'
      );
      const governance = governanceWallet(args, env, provider);
      report.governance_signing_address = normalizeAddress(
        await governance.getAddress(),
        'governance signing address'
      );
      report.signer_is_governance = report.governance_signing_address.toLowerCase()
        === report.governance_signer.toLowerCase();
      const signature = await signMaxEpochDeltaProposal({
        signer: governance,
        pool,
        newMaxEpochDelta: next,
      });
      method = pool.proposeMaxEpochDelta;
      params = [next, signature];
      report.new_max_epoch_delta_wei = next.toString();
    } else if (command === 'execute-max-epoch-delta') {
      method = pool.executeMaxEpochDelta;
      report.pending = pendingMaxRecord(await pool.pendingMaxEpochDelta());
    } else if (command === 'propose-rescue') {
      const to = normalizeAddress(args.to ?? env.MAYHEM_TAP_RESCUE_TO, 'rescue destination');
      const amount = parseNonNegativeBigInt(args['amount-wei'] ?? args.amount, 'rescue amount');
      if (amount === 0n) throw new Error('rescue amount must be positive');
      const governance = governanceWallet(args, env, provider);
      report.governance_signing_address = normalizeAddress(
        await governance.getAddress(),
        'governance signing address'
      );
      report.signer_is_governance = report.governance_signing_address.toLowerCase()
        === report.governance_signer.toLowerCase();
      const signature = await signRescueProposal({ signer: governance, pool, to, amount });
      method = pool.proposeRescue;
      params = [to, amount, signature];
      report.rescue_to = to;
      report.rescue_amount_wei = amount.toString();
      report.rescuable_surplus_wei = (await pool.rescuableSurplus()).toString();
    } else {
      method = pool.executeRescue;
      report.pending = pendingRescueRecord(await pool.pendingRescue());
    }
    report.calldata = pool.interface.encodeFunctionData(method.name, params);
    report.simulation = await simulate(method, params);
    return { provider, pool, method, params, report };
  } catch (error) {
    if (provider.destroy) provider.destroy();
    throw error;
  }
}

export async function runOwnerOp({ args = parseArgs(), env = process.env } = {}) {
  const json = boolArg(args.json, false);
  const confirm = boolArg(args.confirm, false);
  const { provider, pool, method, params, report } = await buildOwnerOpPlan({ args, env });
  try {
    if (confirm) {
      if (!report.simulation.ok) {
        throw new Error(`Dry-run failed; refusing to broadcast: ${report.simulation.error || 'unknown simulation failure'}`);
      }
      const tx = await method(...params);
      const receipt = await tx.wait();
      report.submitted = true;
      report.tx_hash = receipt.hash;
      report.receipt_status = Number(receipt.status);
      report.after_max_epoch_delta_wei = (await pool.maxEpochDelta()).toString();
    }
    if (json) console.log(JSON.stringify(report, null, 2));
    else {
      console.log(report.submitted ? '[tap:owner] submitted' : '[tap:owner] dry-run');
      console.log('[tap:owner] command:', report.command);
      console.log('[tap:owner] pool:', report.pool);
      console.log('[tap:owner] owner:', report.owner);
      console.log('[tap:owner] governance signer:', report.governance_signer);
      console.log('[tap:owner] calldata:', report.calldata);
      console.log('[tap:owner] dry-run ok:', report.simulation.ok);
      if (report.simulation.error) console.log('[tap:owner] dry-run error:', report.simulation.error);
      if (report.tx_hash) console.log('[tap:owner] tx:', report.tx_hash);
    }
    return report;
  } finally {
    if (provider.destroy) provider.destroy();
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  runOwnerOp().catch((error) => {
    console.error('tap-owner-op:', safeErrorMessage(error));
    process.exit(1);
  });
}
