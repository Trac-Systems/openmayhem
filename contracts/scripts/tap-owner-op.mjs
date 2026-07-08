#!/usr/bin/env node
// Owner-only MayhemInferencePool operations. Dry-run by default.
import fs from 'node:fs';
import path from 'node:path';

import { ethers } from 'ethers';

import { DEFAULT_MAINNET_DEPLOYMENT_FILE } from './deploy-mainnet.mjs';

export const POOL_OWNER_ABI = [
  'function owner() view returns (address)',
  'function maxEpochDelta() view returns (uint256)',
  'function setMaxEpochDelta(uint256 newMax)',
];

const poolIface = new ethers.Interface(POOL_OWNER_ABI);

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
  const text = String(value).trim().toLowerCase();
  return ['1', 'true', 'yes', 'on'].includes(text);
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

function privateKeyFromOwnerFile(file) {
  const parsed = readJsonIfExists(file);
  const key = parsed.privateKey ?? parsed.private_key;
  if (!key) return null;
  return String(key).trim();
}

function resolveOwnerPrivateKey(args, env = process.env) {
  const key = args['owner-private-key']
    ?? env.MAYHEM_TAP_POOL_OWNER_PRIVATE_KEY
    ?? privateKeyFromOwnerFile(args['owner-key-file'] ?? env.MAYHEM_TAP_POOL_OWNER_KEY_FILE);
  if (!/^0x[0-9a-fA-F]{64}$/.test(String(key ?? '').trim())) {
    throw new Error('Missing TAP pool owner private key. Set MAYHEM_TAP_POOL_OWNER_KEY_FILE or MAYHEM_TAP_POOL_OWNER_PRIVATE_KEY.');
  }
  return String(key).trim();
}

function errorMessage(error) {
  return error?.shortMessage || error?.reason || error?.message || String(error);
}

export function encodeSetMaxEpochDeltaCalldata(newMax) {
  return poolIface.encodeFunctionData('setMaxEpochDelta', [parseNonNegativeBigInt(newMax, 'max epoch delta')]);
}

export async function setMaxEpochDeltaPlan({
  args = parseArgs(),
  env = process.env,
} = {}) {
  const command = String(args._?.[0] || args.command || 'set-max-epoch-delta');
  if (command !== 'set-max-epoch-delta') throw new Error(`Unsupported owner op: ${command}`);
  const ethRpc = String(args['eth-rpc'] ?? args.rpc ?? env.MAYHEM_TAP_ETH_RPC ?? '').trim();
  if (!ethRpc) throw new Error('Missing Ethereum RPC URL');
  const poolAddress = resolvePoolAddress(args, env);
  const newMax = parseNonNegativeBigInt(args['max-epoch-delta'] ?? args.max ?? env.MAYHEM_TAP_MAX_EPOCH_DELTA, 'max epoch delta');
  const provider = new ethers.JsonRpcProvider(ethRpc);
  try {
    const wallet = new ethers.Wallet(resolveOwnerPrivateKey(args, env), provider);
    const pool = new ethers.Contract(poolAddress, POOL_OWNER_ABI, wallet);
    const [network, owner, currentMax] = await Promise.all([
      provider.getNetwork(),
      pool.owner(),
      pool.maxEpochDelta(),
    ]);
    const signingAddress = await wallet.getAddress();
    const calldata = encodeSetMaxEpochDeltaCalldata(newMax);
    const simulation = {
      static_call_ok: false,
      gas_estimate_ok: false,
      gas_estimate: null,
      error: null,
    };
    try {
      await pool.setMaxEpochDelta.staticCall(newMax);
      simulation.static_call_ok = true;
    } catch (error) {
      simulation.error = errorMessage(error);
    }
    try {
      simulation.gas_estimate = (await pool.setMaxEpochDelta.estimateGas(newMax)).toString();
      simulation.gas_estimate_ok = true;
    } catch (error) {
      simulation.error = simulation.error || errorMessage(error);
    }

    return {
      provider,
      pool,
      report: {
        rail: 'tap',
        command,
        submitted: false,
        chain_id: Number(network.chainId),
        pool: poolAddress,
        owner: normalizeAddress(owner, 'owner()'),
        signing_address: normalizeAddress(signingAddress, 'signing address'),
        signer_is_owner: normalizeAddress(owner, 'owner()').toLowerCase() === normalizeAddress(signingAddress, 'signing address').toLowerCase(),
        current_max_epoch_delta_wei: currentMax.toString(),
        new_max_epoch_delta_wei: newMax.toString(),
        calldata,
        simulation,
      },
      newMax,
    };
  } catch (error) {
    if (provider.destroy) provider.destroy();
    throw error;
  }
}

export async function runOwnerOp({
  args = parseArgs(),
  env = process.env,
} = {}) {
  const json = boolArg(args.json, false);
  const confirm = boolArg(args.confirm, false);
  const { provider, pool, report, newMax } = await setMaxEpochDeltaPlan({ args, env });
  try {
    if (confirm) {
      if (!report.simulation.static_call_ok || !report.simulation.gas_estimate_ok) {
        throw new Error(`Dry-run failed; refusing to broadcast: ${report.simulation.error || 'unknown simulation failure'}`);
      }
      const tx = await pool.setMaxEpochDelta(newMax);
      const receipt = await tx.wait();
      report.submitted = true;
      report.tx_hash = receipt.hash;
      report.receipt_status = receipt.status;
      report.after_max_epoch_delta_wei = (await pool.maxEpochDelta()).toString();
    }
    if (json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log(report.submitted ? '[tap:owner] submitted' : '[tap:owner] dry-run');
      console.log('[tap:owner] pool:', report.pool);
      console.log('[tap:owner] owner:', report.owner);
      console.log('[tap:owner] signing address:', report.signing_address);
      console.log('[tap:owner] signer is owner:', report.signer_is_owner);
      console.log('[tap:owner] current maxEpochDelta:', report.current_max_epoch_delta_wei);
      console.log('[tap:owner] new maxEpochDelta:', report.new_max_epoch_delta_wei);
      console.log('[tap:owner] calldata:', report.calldata);
      console.log('[tap:owner] dry-run ok:', report.simulation.static_call_ok && report.simulation.gas_estimate_ok);
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
    console.error('tap-owner-op:', error?.shortMessage || error?.message || String(error));
    process.exit(1);
  });
}
