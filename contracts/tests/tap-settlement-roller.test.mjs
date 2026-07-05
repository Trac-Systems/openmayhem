import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import { distribution } from '../scripts/merkle.mjs';
import {
  buildTapSettlement,
  encodeSetRootCalldata,
  encodeWithdrawOperatorCalldata,
  guardianPreSignReport,
  muToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';

const TAP_USD_E6 = 1_000_000;
const U = (n) => ethers.parseUnits(String(n), 18);
const SCRIPT_PATH = fileURLToPath(new URL('../scripts/tap-settlement-roller.mjs', import.meta.url));
const OPERATOR_KEY = `0x${'11'.repeat(32)}`;
const BUYER_KEY = `0x${'22'.repeat(32)}`;
const PROVIDER_KEY = `0x${'33'.repeat(32)}`;
const GANACHE_BALANCE = ethers.toBeHex(ethers.parseEther('100'));

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
    }, 20_000);
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

function receipt({
  session,
  provider,
  user = 'user-a',
  mu,
  seq = 1,
  extra = {},
}) {
  return {
    receipt: {
      body: {
        session_id: session,
        seq,
        user,
        provider,
        mu_owed_cum: mu,
      },
    },
    ...extra,
  };
}

test('TAP settlement roller posts root and provider proof verifies independently', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 5 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const providerB = await provider.getSigner(3);
  const operatorTreasury = await provider.getSigner(4);
  const { token, pool, poolAddr } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const providerAccounts = {
    provider_a: await providerA.getAddress(),
    provider_b: await providerB.getAddress(),
  };
  const bundle = {
    receipts: [
      receipt({ session: 's1', provider: 'provider_a', mu: 1_000_000 }),
      receipt({ session: 's2', provider: 'provider_b', mu: 3_000_000 }),
    ],
  };
  const rolled = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdE6: TAP_USD_E6,
    ledgerFeeBps: 1500,
    pool,
    ownerSigner: operator,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });

  const spentA = muToTapWei(1_000_000, TAP_USD_E6);
  const spentB = muToTapWei(3_000_000, TAP_USD_E6);
  const claimA = providerShareWei(spentA);
  const claimB = providerShareWei(spentB);
  const expectedDist = distribution([
    { account: providerAccounts.provider_a.toLowerCase(), amount: claimA },
    { account: providerAccounts.provider_b.toLowerCase(), amount: claimB },
  ]);

  assert.equal(rolled.posted, true);
  assert.equal(rolled.epoch, 1);
  assert.equal(rolled.cumulative_spent_wei, (spentA + spentB).toString());
  assert.equal(rolled.provider_claimed_wei, (claimA + claimB).toString());
  assert.equal(rolled.root, expectedDist.root);
  assert.equal(await pool.epoch(), 1n);
  assert.equal(await pool.cumulativeSpent(), spentA + spentB);
  assert.equal(await pool.merkleRoot(), expectedDist.root);
  assert.equal(rolled.operator_fee.auto_sent, true);
  assert.equal(rolled.operator_fee.predicted_claimable_wei, ((spentA + spentB) * 1500n / 10_000n).toString());
  assert.equal(rolled.operator_fee.actual_claimable_wei, rolled.operator_fee.predicted_claimable_wei);
  assert.match(rolled.operator_fee.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(await token.balanceOf(await operatorTreasury.getAddress()), (spentA + spentB) * 1500n / 10_000n);
  assert.equal(await pool.operatorClaimable(), 0n);

  await (await pool.connect(providerA).claim(
    providerAccounts.provider_a,
    claimA,
    rolled.proofs[providerAccounts.provider_a.toLowerCase()].proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccounts.provider_a), claimA);
});

test('TAP settlement roller uses provider account mapping and skips repeated roots', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const operatorTreasury = await provider.getSigner(3);
  const { token, pool, poolAddr } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();

  const bundle = {
    receipts: [receipt({ session: 's1', provider: 'provider_a', mu: 1_000_000 })],
  };
  assert.throws(
    () => buildTapSettlement({ bundle, tapUsdE6: TAP_USD_E6, ledgerFeeBps: 1500 }),
    /Missing TAP claim address/
  );
  assert.throws(
    () => buildTapSettlement({
      bundle,
      providerAccounts: { provider_a: '0x1111111111111111111111111111111111111111' },
      tapUsdE6: TAP_USD_E6,
      ledgerFeeBps: 1200,
    }),
    /must equal on-chain OPERATOR_BPS/
  );

  const providerAccounts = { provider_a: await providerA.getAddress() };
  const first = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdE6: TAP_USD_E6,
    ledgerFeeBps: 1500,
    pool,
    ownerSigner: operator,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });
  assert.equal(first.posted, true);

  const replay = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdE6: TAP_USD_E6,
    ledgerFeeBps: 1500,
    pool,
    ownerSigner: operator,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });
  assert.equal(replay.posted, false);
  assert.equal(replay.blocked, true);
  assert.deepEqual(replay.reasons, ['no new spend since last root']);
});

test('TAP settlement roller supports weighted multi-provider receipts', () => {
  const a = '0x1111111111111111111111111111111111111111';
  const b = '0x2222222222222222222222222222222222222222';
  const settlement = buildTapSettlement({
    tapUsdE6: TAP_USD_E6,
    ledgerFeeBps: 1500,
    bundle: {
      receipts: [{
        receipt: {
          body: {
            session_id: 's1',
            seq: 1,
            user: 'user-a',
            provider: 'ignored-primary',
            provider_refs: ['pa', 'pb'],
            contribution_weights_bps: [2_500, 7_500],
            mu_owed_cum: 4_000_000,
          },
        },
      }],
    },
    providerAccounts: { pa: a, pb: b },
  });
  const spent = muToTapWei(4_000_000, TAP_USD_E6);
  assert.equal(settlement.cumulative_spent_wei, spent.toString());
  assert.equal(settlement.proofs[a].cumulative_wei, providerShareWei(spent, 2_500).toString());
  assert.equal(settlement.proofs[b].cumulative_wei, providerShareWei(spent, 7_500).toString());
});

test('guardian pre-sign screen halts invariant violations', async () => {
  const a = '0x1111111111111111111111111111111111111111';
  const root = `0x${'11'.repeat(32)}`;

  const overAllocated = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '751' }],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '1000',
    maxEpochDeltaWei: '0',
  });
  assert.equal(overAllocated.ok, false);
  assert.match(overAllocated.reasons.join('; '), /provider owed > 75% spent cap/);
  assert.match(overAllocated.reasons.join('; '), /owed \+ operator cap \+ burn cap > deposited/);

  const spentPastDeposits = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1001',
      entries: [{ account: a, cumulative_wei: '750' }],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '1000',
    maxEpochDeltaWei: '0',
  });
  assert.equal(spentPastDeposits.ok, false);
  assert.match(spentPastDeposits.reasons.join('; '), /spent > deposited/);

  const capExceeded = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '2000',
      entries: [{ account: a, cumulative_wei: '1500' }],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '2000',
    maxEpochDeltaWei: '1999',
  });
  assert.equal(capExceeded.ok, false);
  assert.match(capExceeded.reasons.join('; '), /epoch delta > cap/);

  const decreasedProvider = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '800' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
  });
  assert.equal(decreasedProvider.ok, false);
  assert.match(decreasedProvider.reasons.join('; '), /cumulative for .* decreased/);
});

test('TAP settlement CLI dry-runs and broadcasts with env key against a locked JSON-RPC node', async (t) => {
  const server = Ganache.server({
    logging: { quiet: true },
    chain: { chainId: 61_001 },
    wallet: {
      lock: true,
      accounts: [
        { secretKey: OPERATOR_KEY, balance: GANACHE_BALANCE },
        { secretKey: BUYER_KEY, balance: GANACHE_BALANCE },
        { secretKey: PROVIDER_KEY, balance: GANACHE_BALANCE },
      ],
    },
  });
  await new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', (error) => (error ? reject(error) : resolve()));
  });
  let provider = null;
  t.after(() => {
    if (provider?.destroy) provider.destroy();
    try {
      const closing = server.close();
      if (closing?.catch) closing.catch(() => {});
    } catch (_error) {
      // Best-effort cleanup for Ganache's mixed callback/promise close API in node:test.
    }
  });
  const rpc = `http://127.0.0.1:${server.address().port}`;
  provider = new ethers.JsonRpcProvider(rpc);
  const operator = new ethers.NonceManager(new ethers.Wallet(OPERATOR_KEY, provider));
  const buyer = new ethers.Wallet(BUYER_KEY, provider);
  const providerSigner = new ethers.Wallet(PROVIDER_KEY, provider);
  const operatorTreasury = ethers.Wallet.createRandom().connect(provider);
  const { token, pool, poolAddr } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-roller-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const bundlePath = path.join(tmp, 'bundle.json');
  const providerAccountsPath = path.join(tmp, 'providers.json');
  fs.writeFileSync(bundlePath, JSON.stringify({
    receipts: [receipt({ session: 'cli-s1', provider: 'provider_cli', mu: 1_000_000 })],
  }, null, 2));
  fs.writeFileSync(providerAccountsPath, JSON.stringify({
    provider_cli: await providerSigner.getAddress(),
  }, null, 2));

  const baseArgs = [
    SCRIPT_PATH,
    '--bundle', bundlePath,
    '--provider-accounts', providerAccountsPath,
    '--tap-usd-e6', String(TAP_USD_E6),
    '--ledger-fee-bps', '1500',
    '--eth-rpc', rpc,
    '--pool', poolAddr,
    '--operator-address', await operatorTreasury.getAddress(),
    '--json',
  ];
  const baseEnv = { ...process.env };
  delete baseEnv.MAYHEM_TAP_ROLLER_PRIVATE_KEY;
  const missingKey = await runNode(baseArgs, {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: baseEnv,
  });
  assert.notEqual(missingKey.status, 0);
  assert.match(missingKey.stderr, /MAYHEM_TAP_ROLLER_PRIVATE_KEY/);

  const dryRun = await runNode(baseArgs, {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: { ...baseEnv, MAYHEM_TAP_ROLLER_PRIVATE_KEY: OPERATOR_KEY },
  });
  assert.equal(dryRun.status, 0, dryRun.stderr);
  const report = JSON.parse(dryRun.stdout);
  assert.equal(report.posted, false);
  assert.equal(report.signer_env, 'MAYHEM_TAP_ROLLER_PRIVATE_KEY');
  assert.equal(report.signing_address, (await operator.getAddress()).toLowerCase());
  assert.equal(report.set_root_dry_run.ok, true);
  assert.equal(report.set_root_dry_run.static_call_ok, true);
  assert.match(report.set_root_dry_run.gas_estimate, /^[0-9]+$/);
  assert.equal(report.set_root_calldata, encodeSetRootCalldata({
    root: report.root,
    epoch: report.epoch,
    cumulativeSpentWei: report.cumulative_spent_wei,
  }));
  assert.equal(report.operator_fee.destination, (await operatorTreasury.getAddress()).toLowerCase());
  assert.equal(report.operator_fee.predicted_claimable_wei, (muToTapWei(1_000_000, TAP_USD_E6) * 1500n / 10_000n).toString());
  assert.equal(report.operator_fee.calldata, encodeWithdrawOperatorCalldata({
    to: await operatorTreasury.getAddress(),
    amountWei: report.operator_fee.predicted_claimable_wei,
  }));
  assert.equal(report.operator_fee.auto_sent, false);
  assert.match(report.copy_paste_confirm_command, /--confirm/);
  assert.match(report.copy_paste_confirm_command, /--operator-address/);
  assert.doesNotMatch(JSON.stringify(report), new RegExp(OPERATOR_KEY.slice(2), 'i'));
  assert.equal(await pool.epoch(), 0n);

  const confirmed = await runNode([...baseArgs, '--confirm'], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: { ...baseEnv, MAYHEM_TAP_ROLLER_PRIVATE_KEY: OPERATOR_KEY },
  });
  assert.equal(confirmed.status, 0, confirmed.stderr);
  const posted = JSON.parse(confirmed.stdout);
  assert.equal(posted.posted, true);
  assert.match(posted.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.signing_address, (await operator.getAddress()).toLowerCase());
  assert.equal(posted.operator_fee.auto_sent, true);
  assert.match(posted.operator_fee.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.operator_fee.actual_claimable_wei, posted.operator_fee.predicted_claimable_wei);
  assert.equal(await token.balanceOf(await operatorTreasury.getAddress()), BigInt(posted.operator_fee.predicted_claimable_wei));
  assert.equal(await pool.operatorClaimable(), 0n);
  assert.equal(await pool.epoch(), 1n);
});
