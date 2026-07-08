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
import { deployPoolWithToken } from '../scripts/deploy-local.mjs';

const SCRIPT = fileURLToPath(new URL('../scripts/tap-owner-op.mjs', import.meta.url));
const DEPLOYER_KEY = `0x${'55'.repeat(32)}`;
const OWNER_KEY = `0x${'66'.repeat(32)}`;
const NOT_OWNER_KEY = `0x${'77'.repeat(32)}`;
const GANACHE_BALANCE = ethers.toBeHex(ethers.parseEther('100'));
const U = (n) => ethers.parseUnits(String(n), 18);

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

test('TAP owner op dry-runs setMaxEpochDelta and refuses non-owner broadcast', async (t) => {
  const server = Ganache.server({
    logging: { quiet: true },
    chain: { chainId: 61_003 },
    wallet: {
      lock: true,
      accounts: [
        { secretKey: DEPLOYER_KEY, balance: GANACHE_BALANCE },
        { secretKey: OWNER_KEY, balance: GANACHE_BALANCE },
        { secretKey: NOT_OWNER_KEY, balance: GANACHE_BALANCE },
      ],
    },
  });
  await new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', (error) => (error ? reject(error) : resolve()));
  });
  t.after(() => {
    try {
      const closing = server.close();
      if (closing?.catch) closing.catch(() => {});
    } catch (_error) {}
  });

  const rpc = `http://127.0.0.1:${server.address().port}`;
  const provider = new ethers.JsonRpcProvider(rpc);
  t.after(() => { if (provider.destroy) provider.destroy(); });
  const deployer = new ethers.NonceManager(new ethers.Wallet(DEPLOYER_KEY, provider));
  const owner = new ethers.Wallet(OWNER_KEY, provider);
  const art = compileAll();
  const token = await new ethers.ContractFactory(art.MockTTAP.abi, art.MockTTAP.bytecode, deployer).deploy();
  await token.waitForDeployment();
  const { pool, poolAddr } = await deployPoolWithToken(
    deployer,
    await token.getAddress(),
    await owner.getAddress(),
    U(500),
    art
  );

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-owner-op-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const ownerKeyFile = path.join(tmp, 'owner.json');
  fs.writeFileSync(ownerKeyFile, JSON.stringify({ address: await owner.getAddress(), privateKey: OWNER_KEY }, null, 2));
  const baseArgs = [
    SCRIPT,
    'set-max-epoch-delta',
    '--eth-rpc', rpc,
    '--pool', poolAddr,
    '--max-epoch-delta', U(250).toString(),
    '--owner-key-file', ownerKeyFile,
    '--json',
  ];

  const dryRun = await runNode(baseArgs);
  assert.equal(dryRun.status, 0, dryRun.stderr);
  const dryReport = JSON.parse(dryRun.stdout);
  assert.equal(dryReport.submitted, false);
  assert.equal(dryReport.signer_is_owner, true);
  assert.equal(dryReport.current_max_epoch_delta_wei, U(500).toString());
  assert.equal(dryReport.new_max_epoch_delta_wei, U(250).toString());
  assert.equal(dryReport.simulation.static_call_ok, true);
  assert.equal(dryReport.simulation.gas_estimate_ok, true);
  assert.match(dryReport.calldata, /^0x/);
  assert.equal((await pool.maxEpochDelta()).toString(), U(500).toString());

  const confirm = await runNode([...baseArgs, '--confirm']);
  assert.equal(confirm.status, 0, confirm.stderr);
  const confirmReport = JSON.parse(confirm.stdout);
  assert.equal(confirmReport.submitted, true);
  assert.equal(confirmReport.after_max_epoch_delta_wei, U(250).toString());
  assert.equal((await pool.maxEpochDelta()).toString(), U(250).toString());

  const notOwner = await runNode([
    SCRIPT,
    'set-max-epoch-delta',
    '--eth-rpc', rpc,
    '--pool', poolAddr,
    '--max-epoch-delta', U(100).toString(),
    '--owner-private-key', NOT_OWNER_KEY,
    '--json',
  ]);
  assert.equal(notOwner.status, 0, notOwner.stderr);
  const notOwnerReport = JSON.parse(notOwner.stdout);
  assert.equal(notOwnerReport.signer_is_owner, false);
  assert.equal(notOwnerReport.simulation.static_call_ok, false);
  assert.match(notOwnerReport.simulation.error, /OwnableUnauthorizedAccount|execution reverted|missing revert data/);

  const notOwnerConfirm = await runNode([
    SCRIPT,
    'set-max-epoch-delta',
    '--eth-rpc', rpc,
    '--pool', poolAddr,
    '--max-epoch-delta', U(100).toString(),
    '--owner-private-key', NOT_OWNER_KEY,
    '--json',
    '--confirm',
  ]);
  assert.notEqual(notOwnerConfirm.status, 0);
  assert.match(notOwnerConfirm.stderr, /refusing to broadcast/i);
  assert.equal((await pool.maxEpochDelta()).toString(), U(250).toString());
});
