import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import { distribution } from '../scripts/merkle.mjs';
import {
  buildTapSettlement,
  muToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';

const TAP_USD_E6 = 1_000_000;
const U = (n) => ethers.parseUnits(String(n), 18);

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
    pool,
    ownerSigner: operator,
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
  const { token, pool, poolAddr } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();

  const bundle = {
    receipts: [receipt({ session: 's1', provider: 'provider_a', mu: 1_000_000 })],
  };
  assert.throws(
    () => buildTapSettlement({ bundle, tapUsdE6: TAP_USD_E6 }),
    /Missing TAP claim address/
  );

  const providerAccounts = { provider_a: await providerA.getAddress() };
  const first = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdE6: TAP_USD_E6,
    pool,
    ownerSigner: operator,
    post: true,
  });
  assert.equal(first.posted, true);

  const replay = await rollTapSettlement({
    bundle,
    providerAccounts,
    tapUsdE6: TAP_USD_E6,
    pool,
    ownerSigner: operator,
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
