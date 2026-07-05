import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import {
  buildTapClaimCalldata,
  buildTapDepositCalldata,
} from '../scripts/tap-calldata-builder.mjs';
import { claimProofForAccount } from '../scripts/tap-claim-proof.mjs';
import { deployPool } from '../scripts/deploy-local.mjs';
import {
  muToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';

const TAP_USD_E6 = 1_000_000;
const U = (n) => ethers.parseUnits(String(n), 18);

function receipt({ session, provider, epoch, user = 'user-a', mu, seq = 1 }) {
  return {
    epoch,
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

test('TAP loop holds immature earnings, then provider self-claims without a custodial payout wallet', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 5 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const buyerAddress = await buyer.getAddress();
  const providerAccount = await providerA.getAddress();
  const { token, pool, tokenAddr, poolAddr } = await deployPool(operator);

  await (await token.mint(buyerAddress, U(3))).wait();
  const depositIntent = buildTapDepositCalldata({
    from: buyerAddress,
    amountTap: '3',
    token: tokenAddr,
    pool: poolAddr,
    chainId: 61_000,
  });
  await (await buyer.sendTransaction(depositIntent.transactions[0])).wait();
  await (await buyer.sendTransaction(depositIntent.transactions[1])).wait();
  assert.equal(await pool.totalDeposited(), U(3));

  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 's1', provider: 'provider_a', epoch: 1, mu: 2_000_000 })],
  };
  const immature = await rollTapSettlement({
    bundle,
    providerAccounts: { provider_a: providerAccount },
    tapUsdE6: TAP_USD_E6,
    settleThroughEpoch: 1,
    challengeEpochs: 1,
    pool,
    ownerSigner: operator,
    post: true,
  });
  assert.equal(immature.posted, false);
  assert.equal(immature.reason, 'no matured provider earnings');
  assert.equal(immature.held_receipt_count, 1);
  assert.equal(immature.held_mu, 2_000_000);
  assert.equal(immature.payout_model, 'non_custodial_claim');
  assert.equal(immature.custodial_wallet, false);
  assert.equal(await pool.epoch(), 0n);

  const matured = await rollTapSettlement({
    bundle,
    providerAccounts: { provider_a: providerAccount },
    tapUsdE6: TAP_USD_E6,
    settleThroughEpoch: 2,
    challengeEpochs: 1,
    pool,
    ownerSigner: operator,
    post: true,
  });
  assert.equal(matured.posted, true);
  assert.equal(matured.held_receipt_count, 0);
  assert.equal(matured.payout_model, 'non_custodial_claim');
  assert.equal(matured.custodial_wallet, false);

  const proof = await claimProofForAccount({
    settlement: matured,
    account: providerAccount,
    pool,
  });
  assert.equal(proof.claimable, true);
  const claimIntent = buildTapClaimCalldata({
    account: proof.account,
    cumulativeWei: proof.cumulative_wei,
    proof: proof.proof,
    pool: poolAddr,
    token: tokenAddr,
    chainId: 61_000,
  });
  assert.equal(claimIntent.server_signs, false);
  await (await providerA.sendTransaction(claimIntent.transaction)).wait();

  const expectedClaim = providerShareWei(muToTapWei(2_000_000, TAP_USD_E6));
  assert.equal(await token.balanceOf(providerAccount), expectedClaim);
  const afterClaim = await claimProofForAccount({
    settlement: matured,
    account: providerAccount,
    pool,
  });
  assert.equal(afterClaim.claimable, false);
  assert.equal(afterClaim.claimable_wei, '0');
});
