import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import { claimProofForAccount } from '../scripts/tap-claim-proof.mjs';
import {
  auToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';
import {
  makeReceiptIdentity,
  signedTapReceipt,
  targetedTapBindingsFor,
} from './helpers/signed-receipt.mjs';

const TAP_USD_AU = '1000000000000000000';
const usdAu = (value) => (BigInt(value) * 1_000_000_000_000_000_000n).toString();
const U = (n) => ethers.parseUnits(String(n), 18);

const receipt = signedTapReceipt;

test('claim-proof returns provider proof that submits to MayhemInferencePool.claim()', async () => {
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
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(3))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(3))).wait();
  await (await pool.connect(buyer).deposit(U(3))).wait();

  const userId = makeReceiptIdentity();
  const providerId = makeReceiptIdentity();
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 's1', user: userId, provider: providerId, au: usdAu(2) })],
    buyer_refunds: [{ user: userId.publicKeyHex, refund_au: usdAu(1) }],
  };
  const providerAccounts = { [providerId.publicKeyHex]: providerAccount };
  const rolled = await rollTapSettlement({
    bundle,
    providerAccounts,
    targetedSessionBindings: targetedTapBindingsFor(bundle, providerAccounts),
    buyerAccounts: { [userId.publicKeyHex]: await buyer.getAddress() },
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operator.getAddress(),
    post: true,
  });
  const expectedClaim = providerShareWei(auToTapWei(usdAu(2), TAP_USD_AU));

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
    proof.epoch,
    proof.account,
    BigInt(proof.cumulative_wei),
    proof.proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccount), expectedClaim);

  const expectedRefund = auToTapWei(usdAu(1), TAP_USD_AU);
  const refundProof = await claimProofForAccount({
    settlement: rolled,
    account: await buyer.getAddress(),
    pool,
  });
  assert.equal(refundProof.claimable, true);
  assert.equal(refundProof.cumulative_wei, expectedRefund.toString());
  assert.equal(refundProof.claimable_wei, expectedRefund.toString());
  await (await pool.connect(buyer).claim(
    refundProof.epoch,
    refundProof.account,
    BigInt(refundProof.cumulative_wei),
    refundProof.proof
  )).wait();
  assert.equal(await token.balanceOf(await buyer.getAddress()), expectedRefund);

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
