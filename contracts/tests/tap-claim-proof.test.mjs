import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import { claimProofForAccount } from '../scripts/tap-claim-proof.mjs';
import {
  muToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';

const TAP_USD_E6 = 1_000_000;
const U = (n) => ethers.parseUnits(String(n), 18);

function receipt({ session, provider, user = 'user-a', mu, seq = 1 }) {
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
  };
}

test('claim-proof returns provider proof that submits to KnowledgePool.claim()', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const providerAccount = await providerA.getAddress();
  const { token, pool, poolAddr } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(3))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(3))).wait();
  await (await pool.connect(buyer).deposit(U(3))).wait();

  const bundle = {
    receipts: [receipt({ session: 's1', provider: 'provider_a', mu: 2_000_000 })],
  };
  const rolled = await rollTapSettlement({
    bundle,
    providerAccounts: { provider_a: providerAccount },
    tapUsdE6: TAP_USD_E6,
    ledgerFeeBps: 1500,
    pool,
    ownerSigner: operator,
    operatorAddress: await operator.getAddress(),
    post: true,
  });
  const expectedClaim = providerShareWei(muToTapWei(2_000_000, TAP_USD_E6));

  const proof = await claimProofForAccount({
    settlement: rolled,
    account: providerAccount,
    pool,
  });
  assert.equal(proof.claimable, true);
  assert.equal(proof.epoch, 1);
  assert.equal(proof.root, rolled.root);
  assert.equal(proof.root_matches_report, true);
  assert.equal(proof.root_matches_chain, true);
  assert.equal(proof.cumulative_wei, expectedClaim.toString());
  assert.equal(proof.claimed_wei, '0');
  assert.equal(proof.claimable_wei, expectedClaim.toString());
  assert.ok(Array.isArray(proof.proof));
  assert.ok(proof.leaf.startsWith('0x'));

  await (await pool.connect(providerA).claim(
    proof.account,
    BigInt(proof.cumulative_wei),
    proof.proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccount), expectedClaim);

  const after = await claimProofForAccount({
    settlement: rolled,
    account: providerAccount,
    pool,
  });
  assert.equal(after.claimable, false);
  assert.equal(after.reason, 'nothing to claim');
  assert.equal(after.claimed_wei, expectedClaim.toString());
  assert.equal(after.claimable_wei, '0');
});

test('claim-proof refuses a tampered settlement root', async () => {
  const account = '0x1111111111111111111111111111111111111111';
  const report = {
    epoch: 1,
    root: `0x${'11'.repeat(32)}`,
    entries: [{ account, cumulative_wei: '123' }],
  };
  const proof = await claimProofForAccount({ settlement: report, account });
  assert.equal(proof.claimable, false);
  assert.equal(proof.root_matches_report, false);
  assert.equal(proof.reason, 'report root does not match rebuilt distribution');
});
