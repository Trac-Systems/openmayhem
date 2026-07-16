import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import { distribution } from '../scripts/merkle.mjs';
import { signRootProposal } from '../scripts/pool-governance.mjs';
import {
  buildTapSettlement,
  encodeBurnCalldata,
  encodeWithdrawOperatorCalldata,
  POOL_SETTLEMENT_ABI,
  guardianPreSignReport,
  auToTapWei,
  providerShareWei,
  resolveProviderAccountsFromLedger,
  resolveTapSettlementRate,
  resolveTapSettlementEpochPolicy,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';
import { makeReceiptIdentity, signedTapReceipt } from './helpers/signed-receipt.mjs';

const TAP_USD_AU = '1000000000000000000';
const usdAu = (value) => (BigInt(value) * 1_000_000_000_000_000_000n).toString();
const U = (n) => ethers.parseUnits(String(n), 18);
const SCRIPT_PATH = fileURLToPath(new URL('../scripts/tap-settlement-roller.mjs', import.meta.url));
const OPERATOR_KEY = `0x${'11'.repeat(32)}`;
const BUYER_KEY = `0x${'22'.repeat(32)}`;
const PROVIDER_KEY = `0x${'33'.repeat(32)}`;
const GANACHE_BALANCE = ethers.toBeHex(ethers.parseEther('100'));
const BURN_SINK = '0x000000000000000000000000000000000000dEaD';

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

const receipt = signedTapReceipt;

test('TAP settlement resolves provider claim addresses only from current admin ledger state', async () => {
  const providerId = makeReceiptIdentity();
  const providerAccount = ethers.Wallet.createRandom().address;
  const admin = 'aa'.repeat(32);
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 'ledger-payout', provider: providerId, au: usdAu(1) })],
  };
  const state = new Map([
    ['admin', admin],
    [`prov/${providerId.publicKeyHex}`, {
      status: 'active',
      payouts: {
        tap: {
          method: 'tap',
          addr: providerAccount,
          set_by: admin,
          set_by_role: 'admin',
        },
      },
    }],
  ]);
  const fetchImpl = async (url) => ({
    ok: true,
    json: async () => ({ value: state.get(new URL(url).searchParams.get('key')) ?? null }),
  });

  assert.deepEqual(
    await resolveProviderAccountsFromLedger({
      bundle,
      peerRpcUrl: 'http://127.0.0.1:49223/v1',
      fetchImpl,
    }),
    { [providerId.publicKeyHex]: providerAccount.toLowerCase() }
  );

  state.get(`prov/${providerId.publicKeyHex}`).payouts.tap.set_by = 'bb'.repeat(32);
  await assert.rejects(
    resolveProviderAccountsFromLedger({
      bundle,
      peerRpcUrl: 'http://127.0.0.1:49223/v1',
      fetchImpl,
    }),
    /current admin/
  );
});

test('TAP settlement rate lock survives oracle updates and rejects a different bundle', async (t) => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-rate-lock-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const lockPath = path.join(tmp, 'epoch-1.tap-rate.json');
  const admin = 'aa'.repeat(32);
  const rateKey = `rate/tap/3600/${'bb'.repeat(32)}`;
  const bundle = { epoch: 1, receipts: [] };
  let fetches = 0;
  const fetchImpl = async (url) => {
    fetches += 1;
    const key = new URL(url).searchParams.get('key');
    return {
      ok: true,
      async json() {
        if (key === 'admin') return { value: admin };
        if (key === 'tap/rate/latest') {
          return {
            value: {
              denom: 'tap_usd_au',
              tap_usd_au: TAP_USD_AU,
              source: 'uniswap-v2-twap-median',
              ts: 3_600,
              updated_at: rateKey,
              posted_by: admin,
              posted_by_role: 'admin',
            },
          };
        }
        return { value: null };
      },
    };
  };

  const first = await resolveTapSettlementRate({
    bundle,
    tapRateLockPath: lockPath,
    peerRpcUrl: 'http://127.0.0.1:1/v1',
    fetchImpl,
  });
  assert.equal(first.tap_usd_au, TAP_USD_AU);
  assert.equal(first.rate_record_key, rateKey);
  assert.equal(fetches, 2);
  assert.equal(fs.statSync(lockPath).mode & 0o777, 0o600);

  const replay = await resolveTapSettlementRate({
    bundle,
    tapRateLockPath: lockPath,
    peerRpcUrl: 'http://127.0.0.1:1/v1',
    fetchImpl: async () => {
      throw new Error('a replay must never refetch mutable tap/rate/latest');
    },
  });
  assert.deepEqual(replay, first);
  await assert.rejects(
    resolveTapSettlementRate({
      bundle: { epoch: 1, receipts: [{ changed: true }] },
      tapRateLockPath: lockPath,
      peerRpcUrl: 'http://127.0.0.1:1/v1',
      fetchImpl,
    }),
    /does not match bundle content/
  );
});

test('TAP settlement reads challenge and maturity epochs from active ledger state', async () => {
  const state = new Map([
    ['epoch/apply/state', { updated_epoch: 19 }],
    ['params/challenge_epochs', {
      current: { value: 8, effective_at: 0 },
      pending: { value: 12, effective_at: Math.floor(Date.now() / 1_000) + 3_600 },
    }],
  ]);
  const fetchImpl = async (url) => ({
    ok: true,
    json: async () => ({ value: state.get(new URL(url).searchParams.get('key')) ?? null }),
  });

  assert.deepEqual(
    await resolveTapSettlementEpochPolicy({
      peerRpcUrl: 'http://127.0.0.1:49223/v1',
      fetchImpl,
    }),
    { settleThroughEpoch: 19, challengeEpochs: 8 }
  );
});

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
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const providerAId = makeReceiptIdentity();
  const providerBId = makeReceiptIdentity();
  const providerAccounts = {
    [providerAId.publicKeyHex]: await providerA.getAddress(),
    [providerBId.publicKeyHex]: await providerB.getAddress(),
  };
  const bundle = {
    epoch: 1,
    receipts: [
      receipt({ session: 's1', provider: providerAId, au: usdAu(1) }),
      receipt({ session: 's2', provider: providerBId, au: usdAu(3) }),
    ],
  };
  const rolled = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });

  const spentA = auToTapWei(usdAu(1), TAP_USD_AU);
  const spentB = auToTapWei(usdAu(3), TAP_USD_AU);
  const claimA = providerShareWei(spentA);
  const claimB = providerShareWei(spentB);
  const expectedDist = distribution([
    { account: providerAccounts[providerAId.publicKeyHex].toLowerCase(), amount: claimA },
    { account: providerAccounts[providerBId.publicKeyHex].toLowerCase(), amount: claimB },
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
  assert.equal(rolled.burn.auto_sent, true);
  assert.equal(rolled.burn.completed, true);
  assert.equal(rolled.burn.predicted_claimable_wei, ((spentA + spentB) * 1000n / 10_000n).toString());
  assert.equal(rolled.burn.actual_claimable_wei, rolled.burn.predicted_claimable_wei);
  assert.equal(rolled.burn.calldata, encodeBurnCalldata());
  assert.match(rolled.burn.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(await token.balanceOf(BURN_SINK), (spentA + spentB) * 1000n / 10_000n);
  assert.equal(await pool.burnClaimable(), 0n);

  await (await pool.connect(providerA).claim(
    rolled.epoch,
    providerAccounts[providerAId.publicKeyHex],
    claimA,
    rolled.proofs[providerAccounts[providerAId.publicKeyHex].toLowerCase()].proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccounts[providerAId.publicKeyHex]), claimA);
});

test('TAP settlement roller includes buyer refund leaves in the claim root', async () => {
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
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const userId = makeReceiptIdentity();
  const providerId = makeReceiptIdentity();
  const providerAccount = await providerA.getAddress();
  const buyerAccount = await buyer.getAddress();
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 'refund-s1', user: userId, provider: providerId, au: usdAu(4) })],
    buyer_refunds: [{ user: userId.publicKeyHex, refund_au: usdAu(6) }],
  };
  const rolled = await rollTapSettlement({
    bundle,
    providerAccounts: { [providerId.publicKeyHex]: providerAccount },
    buyerAccounts: { [userId.publicKeyHex]: buyerAccount },
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });

  const spentWei = auToTapWei(usdAu(4), TAP_USD_AU);
  const providerClaim = providerShareWei(spentWei);
  const buyerRefund = auToTapWei(usdAu(6), TAP_USD_AU);
  const expectedDist = distribution([
    { account: providerAccount.toLowerCase(), amount: providerClaim },
    { account: buyerAccount.toLowerCase(), amount: buyerRefund },
  ]);

  assert.equal(rolled.posted, true);
  assert.equal(rolled.cumulative_spent_wei, spentWei.toString());
  assert.equal(rolled.provider_claimed_wei, providerClaim.toString());
  assert.equal(rolled.buyer_refund_wei, buyerRefund.toString());
  assert.equal(rolled.total_claimed_wei, (providerClaim + buyerRefund).toString());
  assert.equal(rolled.providers.length, 1);
  assert.equal(rolled.refunds.length, 1);
  assert.equal(rolled.root, expectedDist.root);

  await (await pool.connect(providerA).claim(
    rolled.epoch,
    providerAccount,
    providerClaim,
    rolled.proofs[providerAccount.toLowerCase()].proof
  )).wait();
  await (await pool.connect(buyer).claim(
    rolled.epoch,
    buyerAccount,
    buyerRefund,
    rolled.proofs[buyerAccount.toLowerCase()].proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccount), providerClaim);
  assert.equal(await token.balanceOf(buyerAccount), buyerRefund);
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
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();

  const providerId = makeReceiptIdentity();
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 's1', provider: providerId, au: usdAu(1) })],
  };
  assert.throws(
    () => buildTapSettlement({ bundle, tapUsdAu: TAP_USD_AU, ledgerFeeBps: 1500, settleThroughEpoch: 7 }),
    /Missing TAP claim address/
  );
  assert.throws(
    () => buildTapSettlement({
      bundle,
      providerAccounts: { [providerId.publicKeyHex]: '0x1111111111111111111111111111111111111111' },
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 1,
      challengeEpochs: 0,
    }),
    /challenge_epochs must be non-zero/
  );
  assert.throws(
    () => buildTapSettlement({
      bundle,
      providerAccounts: { [providerId.publicKeyHex]: '0x1111111111111111111111111111111111111111' },
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1200,
      settleThroughEpoch: 7,
    }),
    /must equal on-chain OPERATOR_BPS/
  );

  const providerAccounts = { [providerId.publicKeyHex]: await providerA.getAddress() };
  const first = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });
  assert.equal(first.posted, true);

  const replay = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });
  assert.equal(replay.posted, false);
  assert.equal(replay.root_confirmed, true);
  assert.equal(replay.root_already_posted, true);
  assert.equal(replay.blocked, undefined);
  assert.equal(replay.operator_fee.completed, true);
  assert.equal(replay.burn.completed, true);
});

test('TAP settlement roller resumes fee and burn after an exact root-only partial run', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerSigner = await provider.getSigner(2);
  const operatorTreasury = await provider.getSigner(3);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();

  const providerId = makeReceiptIdentity();
  const providerAccounts = { [providerId.publicKeyHex]: await providerSigner.getAddress() };
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 'partial-root', provider: providerId, au: usdAu(1) })],
  };
  const settlement = buildTapSettlement({
    bundle,
    providerAccounts,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  const governanceSignature = await signRootProposal({
    signer: governanceWallet,
    pool,
    merkleRoot: settlement.root,
    newEpoch: 1,
    newCumulativeSpent: BigInt(settlement.cumulative_spent_wei),
  });
  await (await pool.proposeRoot(
    settlement.root,
    1,
    BigInt(settlement.cumulative_spent_wei),
    governanceSignature
  )).wait();
  await (await pool.executeRoot()).wait();
  assert((await pool.operatorClaimable()) > 0n);
  assert((await pool.burnClaimable()) > 0n);

  const resumed = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    post: true,
  });

  assert.equal(resumed.posted, false);
  assert.equal(resumed.root_confirmed, true);
  assert.equal(resumed.root_already_posted, true);
  assert.equal(resumed.propose_root_dry_run.skipped, true);
  assert.equal(resumed.operator_fee.auto_sent, true);
  assert.equal(resumed.operator_fee.completed, true);
  assert.equal(resumed.burn.auto_sent, true);
  assert.equal(resumed.burn.completed, true);
  assert.equal(await pool.operatorClaimable(), 0n);
  assert.equal(await pool.burnClaimable(), 0n);
});

test('TAP settlement roller refuses unsigned multi-provider split controls', () => {
  const providerId = makeReceiptIdentity();
  const a = '0x1111111111111111111111111111111111111111';
  const b = '0x2222222222222222222222222222222222222222';
  assert.throws(
    () => buildTapSettlement({
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      bundle: {
        epoch: 1,
        receipts: [receipt({
          session: 's1',
          provider: providerId,
          au: usdAu(4),
          extraBody: {
            provider_refs: ['pa', 'pb'],
            contribution_weights_bps: [2_500, 7_500],
          },
        })],
      },
      providerAccounts: { [providerId.publicKeyHex]: a, pa: a, pb: b },
      settleThroughEpoch: 7,
    }),
    /multi-provider TAP receipts require a signed contribution schema/
  );
});

test('TAP settlement roller rejects unsigned and tampered receipts before root construction', () => {
  const providerId = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const signed = receipt({ session: 's1', provider: providerId, au: usdAu(2) });
  assert.throws(
    () => buildTapSettlement({
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      bundle: {
        epoch: 1,
        receipts: [{
          receipt: {
            body: signed.receipt.body,
          },
        }],
      },
      providerAccounts: { [providerId.publicKeyHex]: account },
      settleThroughEpoch: 7,
    }),
    /Invalid enclave receipt signature/
  );

  const tampered = structuredClone(signed);
  tampered.receipt.body.au_owed_cum = usdAu(3);
  assert.throws(
    () => buildTapSettlement({
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      bundle: { epoch: 1, receipts: [tampered] },
      providerAccounts: { [providerId.publicKeyHex]: account },
      settleThroughEpoch: 7,
    }),
    /Invalid enclave receipt signature/
  );
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

  const b = '0x2222222222222222222222222222222222222222';
  const refundWithinEscrow = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1000',
      providers: [{ account: a, cumulative_wei: '750' }],
      refunds: [{ account: b, cumulative_wei: '100' }],
      entries: [
        { account: a, cumulative_wei: '750' },
        { account: b, cumulative_wei: '100' },
      ],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
  });
  assert.equal(refundWithinEscrow.ok, true);
  assert.equal(refundWithinEscrow.provider_owed_wei, '750');
  assert.equal(refundWithinEscrow.total_owed_wei, '850');

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

  const droppedWithoutClaimCheck = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
  });
  assert.equal(droppedWithoutClaimCheck.ok, false);
  assert.match(droppedWithoutClaimCheck.reasons.join('; '), /on-chain claimed check required/);

  const droppedUnclaimed = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
    pool: { claimed: async () => 100n },
  });
  assert.equal(droppedUnclaimed.ok, false);
  assert.match(droppedUnclaimed.reasons.join('; '), /dropped below unclaimed prior/);

  const droppedAfterFullyClaimed = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
    pool: { claimed: async () => 700n },
  });
  assert.equal(droppedAfterFullyClaimed.ok, true);
});

test('TAP settlement CLI dry-runs and broadcasts with env key against a locked JSON-RPC node', async (t) => {
  const server = Ganache.server({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
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
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-roller-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const bundlePath = path.join(tmp, 'bundle.json');
  const providerId = makeReceiptIdentity();
  fs.writeFileSync(bundlePath, JSON.stringify({
    epoch: 1,
    receipts: [receipt({ session: 'cli-s1', provider: providerId, au: usdAu(1) })],
  }, null, 2));
  const admin = 'aa'.repeat(32);
  const ledgerState = new Map([
    ['admin', admin],
    ['epoch/apply/state', { updated_epoch: 7 }],
    ['params/challenge_epochs', { current: { value: 6, effective_at: 0 }, pending: null }],
    ['tap/rate/latest', {
      denom: 'tap_usd_au',
      tap_usd_au: String(TAP_USD_AU),
      source: 'uniswap-v2-twap-median',
      ts: 3_600,
      updated_at: `rate/tap/3600/${'bb'.repeat(32)}`,
      posted_by: admin,
      posted_by_role: 'admin',
    }],
    [`prov/${providerId.publicKeyHex}`, {
      status: 'active',
      payouts: {
        tap: {
          method: 'tap',
          addr: providerSigner.address,
          set_by: admin,
          set_by_role: 'admin',
        },
      },
    }],
  ]);
  const ledgerServer = http.createServer((request, response) => {
    const key = new URL(request.url, 'http://127.0.0.1').searchParams.get('key');
    const value = ledgerState.get(key) ?? null;
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ value }));
  });
  await new Promise((resolve) => ledgerServer.listen(0, '127.0.0.1', resolve));
  t.after(() => ledgerServer.close());
  const ledgerRpc = `http://127.0.0.1:${ledgerServer.address().port}/v1`;

  const baseArgs = [
    SCRIPT_PATH,
    '--bundle', bundlePath,
    '--peer-rpc', ledgerRpc,
    '--tap-rate-lock', path.join(tmp, 'epoch-1.tap-rate.json'),
    '--ledger-fee-bps', '1500',
    '--eth-rpc', rpc,
    '--pool', poolAddr,
    '--operator-address', await operatorTreasury.getAddress(),
    '--json',
  ];
  const baseEnv = { ...process.env };
  delete baseEnv.MAYHEM_TAP_ROLLER_PRIVATE_KEY;
  delete baseEnv.MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY;
  const signingEnv = {
    ...baseEnv,
    MAYHEM_TAP_ROLLER_PRIVATE_KEY: OPERATOR_KEY,
    MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY: governanceWallet.privateKey,
  };
  const localPolicyOverride = await runNode([...baseArgs, '--challenge-epochs', '1'], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.notEqual(localPolicyOverride.status, 0);
  assert.match(localPolicyOverride.stderr, /active admin ledger state/);

  const rawRateOverride = await runNode([...baseArgs, '--tap-usd-au', TAP_USD_AU], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.notEqual(rawRateOverride.status, 0);
  assert.match(rawRateOverride.stderr, /not supported.*rate-lock/i);

  const missingKey = await runNode(baseArgs, {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: baseEnv,
  });
  assert.notEqual(missingKey.status, 0);
  assert.match(missingKey.stderr, /MAYHEM_TAP_ROLLER_PRIVATE_KEY/);

  const dryRun = await runNode(baseArgs, {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.equal(dryRun.status, 0, dryRun.stderr);
  const report = JSON.parse(dryRun.stdout);
  assert.equal(report.posted, false);
  assert.equal(report.tap_rate_lock.tap_usd_au, TAP_USD_AU);
  assert.equal(report.tap_rate_lock.epoch, 1);
  assert.equal(report.signer_env, 'MAYHEM_TAP_ROLLER_PRIVATE_KEY');
  assert.equal(report.signing_address, (await operator.getAddress()).toLowerCase());
  assert.equal(report.governance_signer_env, 'MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY');
  assert.equal(report.governance_signing_address, governanceWallet.address.toLowerCase());
  assert.equal(report.propose_root_dry_run.ok, true);
  assert.equal(report.propose_root_dry_run.static_call_ok, true);
  assert.match(report.propose_root_dry_run.gas_estimate, /^[0-9]+$/);
  const proposal = new ethers.Interface(POOL_SETTLEMENT_ABI)
    .decodeFunctionData('proposeRoot', report.propose_root_calldata);
  assert.equal(proposal.newRoot.toLowerCase(), report.root.toLowerCase());
  assert.equal(proposal.newEpoch, BigInt(report.epoch));
  assert.equal(proposal.newCumulativeSpent, BigInt(report.cumulative_spent_wei));
  assert.match(proposal.governanceSignature, /^0x[0-9a-f]+$/i);
  assert.equal(report.operator_fee.destination, (await operatorTreasury.getAddress()).toLowerCase());
  assert.equal(report.operator_fee.predicted_claimable_wei, (auToTapWei(usdAu(1), TAP_USD_AU) * 1500n / 10_000n).toString());
  assert.equal(report.operator_fee.calldata, encodeWithdrawOperatorCalldata({
    to: await operatorTreasury.getAddress(),
    amountWei: report.operator_fee.predicted_claimable_wei,
  }));
  assert.equal(report.operator_fee.auto_sent, false);
  assert.equal(report.burn.predicted_claimable_wei, (auToTapWei(usdAu(1), TAP_USD_AU) * 1000n / 10_000n).toString());
  assert.equal(report.burn.calldata, encodeBurnCalldata());
  assert.equal(report.burn.auto_sent, false);
  assert.match(report.copy_paste_confirm_command, /--confirm/);
  assert.match(report.copy_paste_confirm_command, /--operator-address/);
  assert.doesNotMatch(
    report.copy_paste_confirm_command,
    /--(?:settle-through-epoch|challenge-epochs|holdback-epochs)/
  );
  assert.doesNotMatch(JSON.stringify(report), new RegExp(OPERATOR_KEY.slice(2), 'i'));
  assert.doesNotMatch(report.copy_paste_replay_command, /--eth-rpc/);
  assert.doesNotMatch(report.copy_paste_replay_command, new RegExp(rpc.replaceAll('.', '\\.')));
  assert.equal(await pool.epoch(), 0n);

  const confirmed = await runNode([...baseArgs, '--confirm'], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.equal(confirmed.status, 0, confirmed.stderr);
  const posted = JSON.parse(confirmed.stdout);
  assert.equal(posted.posted, true, JSON.stringify(posted));
  assert.match(posted.proposal_tx, /^0x[0-9a-f]{64}$/i);
  assert.match(posted.execution_tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.signing_address, (await operator.getAddress()).toLowerCase());
  assert.equal(posted.operator_fee.auto_sent, true);
  assert.match(posted.operator_fee.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.operator_fee.actual_claimable_wei, posted.operator_fee.predicted_claimable_wei);
  assert.equal(await token.balanceOf(await operatorTreasury.getAddress()), BigInt(posted.operator_fee.predicted_claimable_wei));
  assert.equal(await pool.operatorClaimable(), 0n);
  assert.equal(posted.burn.auto_sent, true);
  assert.equal(posted.burn.completed, true);
  assert.match(posted.burn.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.burn.actual_claimable_wei, posted.burn.predicted_claimable_wei);
  assert.equal(await token.balanceOf(BURN_SINK), BigInt(posted.burn.predicted_claimable_wei));
  assert.equal(await pool.burnClaimable(), 0n);
  assert.equal(await pool.epoch(), 1n);
});
