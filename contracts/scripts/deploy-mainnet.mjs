#!/usr/bin/env node
// Real-token deploy of MayhemInferencePool. This never deploys MockTTAP.
//
// Safe by default: dry-run validates the RPC, token contract, signer, owner, gas
// balance, and cap, then exits without broadcasting. Use --confirm only after
// inspecting the plan.
import { ethers } from 'ethers';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { compileAll } from './compile.mjs';
import { deployPoolWithToken } from './deploy-local.mjs';
import {
  TAP_DEPLOYER_SIGNER_ENV,
  walletFromEnv,
} from './signer-env.mjs';

const REPO = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const DEFAULT_MAINNET_DEPLOYMENT_FILE = join(REPO, '.mayhem-local', 'contracts', 'mainnet-addresses.json');

const ERC20_PROBE_ABI = [
  'function decimals() view returns (uint8)',
  'function symbol() view returns (string)',
];

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

function firstEnv(env, names) {
  for (const name of names) {
    const value = env?.[name];
    if (value !== undefined && String(value).trim() !== '') return { name, value: String(value).trim() };
  }
  return null;
}

function redactUrl(value) {
  try {
    const url = new URL(String(value));
    if (url.username) url.username = '***';
    if (url.password) url.password = '***';
    if (url.search) url.search = '';
    const path = url.pathname || '';
    if (path.length > 1) url.pathname = '/...';
    return url.toString();
  } catch (_error) {
    return '<redacted>';
  }
}

function requireEnv(env, names, label) {
  const found = firstEnv(env, names);
  if (!found) throw new Error(`Missing ${label}. Set ${names.join(' or ')}.`);
  return found;
}

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '').trim());
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function parseNonNegativeBigInt(value, label, fallback = 0n) {
  if (value === undefined || value === null || String(value).trim() === '') return fallback;
  try {
    const parsed = BigInt(String(value).trim());
    if (parsed < 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a non-negative integer wei amount`);
  }
}

function deploymentFileFrom(args, env) {
  return String(
    args['deployment-file']
      ?? env.MAYHEM_TAP_DEPLOYMENT_FILE
      ?? DEFAULT_MAINNET_DEPLOYMENT_FILE
  );
}

async function optionalErc20Metadata(provider, tokenAddress) {
  const erc20 = new ethers.Contract(tokenAddress, ERC20_PROBE_ABI, provider);
  let symbol = null;
  let decimals = null;
  try {
    symbol = await erc20.symbol();
  } catch (_error) {
    symbol = null;
  }
  try {
    decimals = Number(await erc20.decimals());
  } catch (_error) {
    decimals = null;
  }
  return { symbol, decimals };
}

function explorerBaseForChain(chainId) {
  if (chainId === 1) return 'https://etherscan.io';
  if (chainId === 11155111) return 'https://sepolia.etherscan.io';
  if (chainId === 17000) return 'https://holesky.etherscan.io';
  return 'https://etherscan.io';
}

async function buildDeploymentPreflight(provider, signer, plan, token, owner, maxEpochDelta) {
  const art = compileAll();
  const factory = new ethers.ContractFactory(
    art.MayhemInferencePool.abi,
    art.MayhemInferencePool.bytecode,
    signer
  );
  const deployTx = await factory.getDeployTransaction(token, owner, maxEpochDelta);
  const nonce = await provider.getTransactionCount(plan.deployer, 'pending');
  const feeData = await provider.getFeeData();
  const feePerGas = feeData.maxFeePerGas ?? feeData.gasPrice ?? 0n;
  const gas = await provider.estimateGas({
    ...deployTx,
    from: plan.deployer,
  });
  return {
    bytecode_bytes: Math.floor(String(art.MayhemInferencePool.bytecode || '').replace(/^0x/, '').length / 2),
    deployer_nonce: nonce,
    predicted_pool_address: ethers.getCreateAddress({ from: plan.deployer, nonce }),
    deploy_gas_estimate: gas.toString(),
    gas_price_wei: feeData.gasPrice?.toString() ?? null,
    max_fee_per_gas_wei: feeData.maxFeePerGas?.toString() ?? null,
    max_priority_fee_per_gas_wei: feeData.maxPriorityFeePerGas?.toString() ?? null,
    estimate_fee_per_gas_wei: feePerGas.toString(),
    estimated_deploy_cost_wei: (gas * feePerGas).toString(),
  };
}

export async function buildDeployPlan({
  args = parseArgs(),
  env = process.env,
} = {}) {
  const rpcEnv = requireEnv(env, ['MAYHEM_TAP_ETH_RPC'], 'Ethereum RPC URL');
  const rpc = rpcEnv.value;
  const tokenEnv = requireEnv(env, ['MAYHEM_TAP_TOKEN_ADDRESS', 'MAYHEM_TAP_TOKEN_ADDR'], 'TAP token address');
  const token = normalizeAddress(tokenEnv.value, tokenEnv.name);
  const provider = new ethers.JsonRpcProvider(rpc);

  try {
    const { envName, wallet } = walletFromEnv(provider, {
      env,
      names: [TAP_DEPLOYER_SIGNER_ENV],
      label: 'TAP deployer private key',
    });
    const deployer = normalizeAddress(await wallet.getAddress(), 'deployer');
    const ownerEnv = firstEnv(env, ['MAYHEM_TAP_POOL_OWNER']);
    const owner = normalizeAddress(ownerEnv?.value || deployer, 'MAYHEM_TAP_POOL_OWNER');
    const maxEpochDelta = parseNonNegativeBigInt(env.MAYHEM_TAP_MAX_EPOCH_DELTA, 'MAYHEM_TAP_MAX_EPOCH_DELTA');
    const allowZeroCap = boolArg(env.MAYHEM_TAP_ALLOW_ZERO_CAP, false);
    if (maxEpochDelta === 0n && !allowZeroCap) {
      throw new Error(
        'MAYHEM_TAP_MAX_EPOCH_DELTA is 0 (no per-epoch spend cap). Set a real ceiling in wei, or MAYHEM_TAP_ALLOW_ZERO_CAP=1 for throwaway local/test deployments.'
      );
    }

    const network = await provider.getNetwork();
    const chainId = Number(network.chainId);
    const tokenCode = await provider.getCode(token);
    if (tokenCode === '0x') {
      throw new Error(`No contract code at TAP token ${token} on chainId ${chainId}`);
    }
    const { symbol, decimals } = await optionalErc20Metadata(provider, token);
    const balanceWei = await provider.getBalance(deployer);
    if (balanceWei === 0n) throw new Error(`Deployer ${deployer} has 0 ETH for gas`);

    return {
      provider,
      signer: wallet,
      plan: {
        contract: 'MayhemInferencePool',
        contract_name: 'MayhemInferencePool.sol:MayhemInferencePool',
        deployment_file: deploymentFileFrom(args, env),
        rpc_env: rpcEnv.name,
        rpc_url_redacted: redactUrl(rpc),
        network: network.name || 'unknown',
        chain_id: chainId,
        explorer_url_base: explorerBaseForChain(chainId),
        token,
        token_env: tokenEnv.name,
        token_symbol: symbol,
        token_decimals: decimals,
        owner,
        deployer,
        signer_env: envName,
        deployer_balance_wei: balanceWei.toString(),
        max_epoch_delta_wei: maxEpochDelta.toString(),
        zero_cap_override: maxEpochDelta === 0n && allowZeroCap,
      },
      maxEpochDelta,
      owner,
      token,
    };
  } catch (error) {
    if (provider.destroy) provider.destroy();
    throw error;
  }
}

function printPlan(plan, { confirm = false } = {}) {
  console.log('\n=== MayhemInferencePool real-token deploy plan ===');
  console.log('  network        :', plan.network, 'chainId', plan.chain_id);
  console.log('  rpc            :', `${plan.rpc_env} (${plan.rpc_url_redacted})`);
  console.log('  token (TAP)    :', plan.token, `symbol ${plan.token_symbol ?? 'UNKNOWN'} decimals ${plan.token_decimals ?? 'UNKNOWN'}`);
  console.log('  pool owner     :', plan.owner, plan.owner.toLowerCase() === plan.deployer.toLowerCase() ? '(= deployer)' : '(MAYHEM_TAP_POOL_OWNER)');
  console.log('  deployer       :', plan.deployer, `balance ${ethers.formatEther(plan.deployer_balance_wei)} ETH`);
  console.log('  signer env     :', plan.signer_env);
  console.log('  maxEpochDelta  :', plan.max_epoch_delta_wei, 'wei', plan.zero_cap_override ? '(ZERO CAP OVERRIDE)' : '');
  if (plan.predicted_pool_address) {
    console.log('  predicted pool :', plan.predicted_pool_address);
    console.log('  deploy gas     :', plan.deploy_gas_estimate, `estimated max cost ${ethers.formatEther(plan.estimated_deploy_cost_wei)} ETH`);
  }
  console.log('  output         :', plan.deployment_file);
  if (plan.chain_id === 1) {
    console.log('  MAINNET        : chainId 1, real funds and irreversible deployment');
  }
  if (plan.token_decimals !== null && plan.token_decimals !== 18) {
    console.log('  WARNING        : TAP token decimals are not 18; check settlement scaling before use');
  }
  if (!confirm) {
    console.log('\nDRY RUN - nothing broadcast. Re-run with --confirm or MAYHEM_TAP_DEPLOY_CONFIRM=1 to deploy.');
  }
}

function writeDeploymentFile(file, payload) {
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(payload, null, 2) + '\n');
}

export async function deployMainnet({
  args = parseArgs(),
  env = process.env,
} = {}) {
  const confirm = boolArg(args.confirm, boolArg(env.MAYHEM_TAP_DEPLOY_CONFIRM, false));
  const json = boolArg(args.json, false);
  const built = await buildDeployPlan({ args, env });
  const { provider, signer, plan, owner, token, maxEpochDelta } = built;

  try {
    const preflight = await buildDeploymentPreflight(provider, signer, plan, token, owner, maxEpochDelta);
    Object.assign(plan, preflight);
    if (!json) printPlan(plan, { confirm });
    if (!confirm) {
      const report = { ...plan, submitted: false, deployed: false };
      if (json) console.log(JSON.stringify(report, null, 2));
      return report;
    }

    const art = compileAll();
    const { pool, poolAddr } = await deployPoolWithToken(signer, token, owner, maxEpochDelta, art);
    const deploymentTx = pool.deploymentTransaction();
    if (!deploymentTx) throw new Error('Deployment transaction is unavailable');
    const deploymentReceipt = await deploymentTx.wait();
    if (!deploymentReceipt || deploymentReceipt.status !== 1) {
      throw new Error(`Deployment transaction ${deploymentTx.hash} did not succeed`);
    }
    const onToken = normalizeAddress(await pool.token(), 'token()');
    const onOwner = normalizeAddress(await pool.owner(), 'owner()');
    const onCap = await pool.maxEpochDelta();
    if (onToken !== token) throw new Error(`Post-deploy token mismatch: ${onToken} != ${token}`);
    if (onOwner !== owner) throw new Error(`Post-deploy owner mismatch: ${onOwner} != ${owner}`);
    if (onCap !== maxEpochDelta) throw new Error(`Post-deploy maxEpochDelta mismatch: ${onCap} != ${maxEpochDelta}`);

    const out = {
      contract: 'MayhemInferencePool',
      contractName: 'MayhemInferencePool.sol:MayhemInferencePool',
      pool: normalizeAddress(poolAddr, 'pool'),
      token,
      owner,
      deployer: plan.deployer,
      signerEnv: plan.signer_env,
      chainId: plan.chain_id,
      rpcEnv: plan.rpc_env,
      rpcUrl: plan.rpc_url_redacted,
      maxEpochDelta: maxEpochDelta.toString(),
      deploymentTxHash: deploymentTx.hash,
      deploymentBlockNumber: deploymentReceipt.blockNumber,
      deploymentGasUsed: deploymentReceipt.gasUsed.toString(),
    };
    writeDeploymentFile(plan.deployment_file, out);

    const report = {
      ...plan,
      submitted: true,
      deployed: true,
      pool: out.pool,
      deployment: out,
      explorer_url: `${plan.explorer_url_base}/address/${out.pool}#code`,
      explorer_tx_url: `${plan.explorer_url_base}/tx/${out.deploymentTxHash}`,
      deployment_tx_hash: out.deploymentTxHash,
      deployment_block_number: out.deploymentBlockNumber,
      deployment_gas_used: out.deploymentGasUsed,
      env_lines: {
        MAYHEM_TAP_TOKEN_ADDR: token,
        MAYHEM_TAP_POOL_ADDRESS: out.pool,
        MAYHEM_TAP_ETH_CHAIN_ID: String(plan.chain_id),
        MAYHEM_TAP_DEPLOYMENT_FILE: plan.deployment_file,
      },
    };
    if (json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log('\n=== deployed ===');
      console.log('  pool           :', out.pool);
      console.log('  token()        :', onToken);
      console.log('  owner()        :', onOwner);
      console.log('  maxEpochDelta():', onCap.toString());
      console.log('  deployment tx  :', out.deploymentTxHash);
      console.log('  wrote          :', plan.deployment_file);
      console.log('\nCopy/paste env lines:');
      for (const [key, value] of Object.entries(report.env_lines)) console.log(`  ${key}=${value}`);
      console.log('\nNext verify command:');
      console.log(`  MAYHEM_TAP_DEPLOYMENT_FILE='${plan.deployment_file}' node contracts/scripts/verify-etherscan.mjs`);
    }
    return report;
  } finally {
    if (provider.destroy) provider.destroy();
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  deployMainnet().catch((error) => {
    console.error('deploy-mainnet:', error?.shortMessage || error?.message || String(error));
    process.exit(1);
  });
}
