import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import {
  buildTapClaimCalldata,
  buildTapDepositCalldata,
  tapAmountToWei,
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

async function localPool(accounts = 5) {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: accounts },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const { token, pool, tokenAddr, poolAddr } = await deployPool(operator);
  return { provider, operator, token, pool, tokenAddr, poolAddr };
}

test('deposit calldata executes approve plus deposit from buyer wallet', async () => {
  const { provider, token, pool, tokenAddr, poolAddr } = await localPool();
  const buyer = await provider.getSigner(1);
  const buyerAddress = await buyer.getAddress();
  await (await token.mint(buyerAddress, U('2'))).wait();

  const intent = buildTapDepositCalldata({
    from: buyerAddress,
    amountTap: '1.25',
    token: tokenAddr,
    pool: poolAddr,
    chainId: 61_000,
  });
  assert.equal(intent.server_signs, false);
  assert.equal(intent.custody, 'external_wallet');
  assert.equal(intent.from, buyerAddress.toLowerCase());
  assert.equal(intent.amount_wei, tapAmountToWei('1.25').toString());
  assert.equal(intent.transactions.length, 2);
  assert.equal(intent.transactions[0].step, 'approve');
  assert.equal(intent.transactions[1].step, 'deposit');

  await (await buyer.sendTransaction(intent.transactions[0])).wait();
  await (await buyer.sendTransaction(intent.transactions[1])).wait();
  assert.equal(await pool.totalDeposited(), tapAmountToWei('1.25'));
});

test('claim calldata executes KnowledgePool.claim from provider wallet', async () => {
  const { provider, operator, token, pool, poolAddr } = await localPool();
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const providerAccount = await providerA.getAddress();

  await (await token.mint(await buyer.getAddress(), U('3'))).wait();
  await (await token.connect(buyer).approve(poolAddr, U('3'))).wait();
  await (await pool.connect(buyer).deposit(U('3'))).wait();

  const rolled = await rollTapSettlement({
    bundle: {
      receipts: [receipt({ session: 's1', provider: 'provider_a', mu: 2_000_000 })],
    },
    providerAccounts: { provider_a: providerAccount },
    tapUsdE6: TAP_USD_E6,
    ledgerFeeBps: 1500,
    pool,
    ownerSigner: operator,
    operatorAddress: await operator.getAddress(),
    post: true,
  });
  const proof = await claimProofForAccount({
    settlement: rolled,
    account: providerAccount,
    pool,
  });
  const claimIntent = buildTapClaimCalldata({
    account: proof.account,
    cumulativeWei: proof.cumulative_wei,
    proof: proof.proof,
    pool: poolAddr,
    token: await token.getAddress(),
    chainId: 61_000,
  });

  assert.equal(claimIntent.server_signs, false);
  assert.equal(claimIntent.from, providerAccount.toLowerCase());
  assert.equal(claimIntent.transaction.to, poolAddr.toLowerCase());
  assert.equal(claimIntent.token, (await token.getAddress()).toLowerCase());
  await (await providerA.sendTransaction(claimIntent.transaction)).wait();

  const expectedClaim = providerShareWei(muToTapWei(2_000_000, TAP_USD_E6));
  assert.equal(await token.balanceOf(providerAccount), expectedClaim);
});
