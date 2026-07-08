import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { compileAll } from '../scripts/compile.mjs';

const DEPLOY_SCRIPT = fileURLToPath(new URL('../scripts/deploy-mainnet.mjs', import.meta.url));
const VERIFY_SCRIPT = fileURLToPath(new URL('../scripts/verify-etherscan.mjs', import.meta.url));
const OPERATOR_KEY = `0x${'44'.repeat(32)}`;
const GANACHE_BALANCE = ethers.toBeHex(ethers.parseEther('100'));
const CAP = ethers.parseUnits('500', 18).toString();

function runNode(args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, args, {
      ...options,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    const timeout = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`child process timed out: ${args.join(' ')}`));
    }, 30_000);
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on('close', (status, signal) => {
      clearTimeout(timeout);
      resolve({ status, signal, stdout, stderr });
    });
  });
}

async function closeServer(server) {
  try {
    await server.close();
  } catch (_error) {
    // Ganache has mixed callback/promise close behavior across versions.
  }
}

test('mainnet deploy and Etherscan verify scripts dry-run and deploy against a locked RPC node', async (t) => {
  const server = Ganache.server({
    logging: { quiet: true },
    chain: { chainId: 61_002 },
    wallet: {
      lock: true,
      accounts: [{ secretKey: OPERATOR_KEY, balance: GANACHE_BALANCE }],
    },
  });
  await new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', (error) => (error ? reject(error) : resolve()));
  });
  t.after(() => closeServer(server));

  const rpc = `http://127.0.0.1:${server.address().port}`;
  const provider = new ethers.JsonRpcProvider(rpc);
  t.after(() => { if (provider.destroy) provider.destroy(); });
  const operator = new ethers.NonceManager(new ethers.Wallet(OPERATOR_KEY, provider));
  const art = compileAll();
  const token = await new ethers.ContractFactory(art.MockTTAP.abi, art.MockTTAP.bytecode, operator).deploy();
  await token.waitForDeployment();
  const tokenAddr = await token.getAddress();

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-mainnet-scripts-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const deploymentFile = path.join(tmp, 'mainnet-addresses.json');
  const baseEnv = {
    ...process.env,
    MAYHEM_TAP_DEPLOYER_PRIVATE_KEY: OPERATOR_KEY,
    MAYHEM_TAP_ETH_RPC: rpc,
    MAYHEM_TAP_TOKEN_ADDR: tokenAddr,
    MAYHEM_TAP_MAX_EPOCH_DELTA: CAP,
    MAYHEM_TAP_DEPLOYMENT_FILE: deploymentFile,
  };

  const dryRun = await runNode([DEPLOY_SCRIPT, '--json'], { env: baseEnv });
  assert.equal(dryRun.status, 0, dryRun.stderr);
  const dryReport = JSON.parse(dryRun.stdout);
  assert.equal(dryReport.deployed, false);
  assert.equal(dryReport.submitted, false);
  assert.equal(dryReport.contract_name, 'MayhemInferencePool.sol:MayhemInferencePool');
  assert.equal(dryReport.token, ethers.getAddress(tokenAddr));
  assert.equal(dryReport.max_epoch_delta_wei, CAP);
  assert.equal(dryReport.rpc, undefined);
  assert.equal(dryReport.rpc_env, 'MAYHEM_TAP_ETH_RPC');
  assert.equal(dryReport.rpc_url_redacted.includes(String(server.address().port)), true);
  assert.equal(ethers.isAddress(dryReport.predicted_pool_address), true);
  assert.equal(BigInt(dryReport.deploy_gas_estimate) > 0n, true);
  assert.equal(BigInt(dryReport.estimated_deploy_cost_wei) > 0n, true);
  assert.equal(Number.isSafeInteger(dryReport.deployer_nonce), true);
  assert.equal(dryReport.bytecode_bytes > 0, true);
  assert.equal(fs.existsSync(deploymentFile), false);

  const deploy = await runNode([DEPLOY_SCRIPT, '--confirm', '--json'], { env: baseEnv });
  assert.equal(deploy.status, 0, deploy.stderr);
  const deployReport = JSON.parse(deploy.stdout);
  assert.equal(deployReport.deployed, true);
  assert.equal(deployReport.submitted, true);
  assert.equal(deployReport.pool, dryReport.predicted_pool_address);
  assert.equal(deployReport.deployment.contractName, 'MayhemInferencePool.sol:MayhemInferencePool');
  assert.equal(fs.existsSync(deploymentFile), true);
  const written = JSON.parse(fs.readFileSync(deploymentFile, 'utf8'));
  assert.equal(written.pool, deployReport.pool);
  assert.equal(written.token, ethers.getAddress(tokenAddr));
  assert.equal(written.maxEpochDelta, CAP);
  assert.equal(written.rpc, undefined);
  assert.equal(written.rpcEnv, 'MAYHEM_TAP_ETH_RPC');

  const pool = new ethers.Contract(written.pool, art.MayhemInferencePool.abi, provider);
  assert.equal(ethers.getAddress(await pool.token()), ethers.getAddress(tokenAddr));
  assert.equal(ethers.getAddress(await pool.owner()), ethers.getAddress(new ethers.Wallet(OPERATOR_KEY).address));
  assert.equal((await pool.maxEpochDelta()).toString(), CAP);

  const verify = await runNode([VERIFY_SCRIPT, '--dry-run', '--json'], {
    env: { ...baseEnv, MAYHEM_ETHERSCAN_API_KEY: 'etherscan-test-key' },
  });
  assert.equal(verify.status, 0, verify.stderr);
  const verifyReport = JSON.parse(verify.stdout);
  assert.equal(verifyReport.submitted, false);
  assert.equal(verifyReport.chain_id, 61_002);
  assert.equal(verifyReport.pool, ethers.getAddress(written.pool));
  assert.equal(verifyReport.contract_name, 'MayhemInferencePool.sol:MayhemInferencePool');
  assert.equal(verifyReport.request.contractname, 'MayhemInferencePool.sol:MayhemInferencePool');
  assert.equal(verifyReport.request.action, 'verifysourcecode');
  assert.equal(verifyReport.request.codeformat, 'solidity-standard-json-input');
  assert.equal(verifyReport.source_file_count >= 1, true);
  assert.equal(verifyReport.constructor_args.length, 64 * 3);
});
