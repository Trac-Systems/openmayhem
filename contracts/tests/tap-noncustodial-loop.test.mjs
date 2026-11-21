import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from '../test-chain/index.mjs';
import { ethers } from 'ethers';

import {
  buildTapClaimCalldata,
  buildTapDepositCalldata,
} from '../scripts/tap-calldata-builder.mjs';
import { claimProofForAccount } from '../scripts/tap-claim-proof.mjs';
import { deployPool } from '../scripts/deploy-local.mjs';
import {
  auToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';
import {
  makeReceiptIdentity,
  signedTapReceipt,
} from './helpers/signed-receipt.mjs';
import { claimSettlementInputs, postAndFinalizeClaimSettlement } from './helpers/tap-claim-settlement.mjs';

const TAP_USD_AU = '1000000000000000000';
const usdAu = (value) => ethers.parseUnits(String(value), 18).toString();
const U = (n) => ethers.parseUnits(String(n), 18);

const receipt = signedTapReceipt;

test('TAP loop holds immature earnings, then provider self-claims without a custodial payout wallet', async (t) => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000, time: new Date(3600_000) },
    wallet: { totalAccounts: 5 },
  });
  t.after(() => ganache.disconnect());
  const provider = new ethers.BrowserProvider(ganache, undefined, { cacheTimeout: -1 });
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const buyerAddress = await buyer.getAddress();
  const providerAccount = await providerA.getAddress();
  const { token, pool, tokenAddr, poolAddr, governanceWallet } = await deployPool(operator);

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

  const providerId = makeReceiptIdentity();
  const bundle = {
    epoch: 1,
    epoch_apply_hash: 'ab'.repeat(32),
    params: { payout_min_au: usdAu(1), fee_bps: 1500, challenge_epochs: 1 },
    receipts: [receipt({ session: 's1', provider: providerId, epoch: 1, au: usdAu(2) })],
  };
  const providerAccounts = { [providerId.publicKeyHex]: providerAccount };
  const args = {
    ...claimSettlementInputs({ bundle, providerAccounts, tapUsdAu: TAP_USD_AU, tokenAddr, poolAddr }),
    settleThroughEpoch: 2,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operator.getAddress(),
    post: true,
  };
  // The ledger holds the entire net provider liability until the challenge epoch passes.
  const [liability] = args.canonicalLiabilities;
  const immature = await rollTapSettlement({
    ...args,
    settleThroughEpoch: 1,
    canonicalLiabilities: [{ ...liability, held_au: liability.total_au }],
  });
  assert.equal(immature.posted, false);
  assert.equal(immature.reason, 'provider earnings await challenge or holdback maturity');
  assert.equal(immature.held_receipt_count, 1);
  assert.equal(immature.payout_bindings[0].au, usdAu(2));
  assert.equal(immature.held_au, usdAu('1.5'));
  assert.deepEqual(immature.entries, []);
  assert.equal(immature.payout_model, 'non_custodial_claim');
  assert.equal(immature.custodial_wallet, false);
  assert.equal(await pool.epoch(), 0n);

  const matured = await postAndFinalizeClaimSettlement(provider, args);
  assert.equal(matured.held_receipt_count, 0);
  assert.equal(matured.cumulative_spent_wei, U(2).toString());
  assert.equal(matured.payout_model, 'non_custodial_claim');
  assert.equal(matured.custodial_wallet, false);
  assert.equal(await pool.operatorWithdrawn(), U('0.3'));
  assert.equal(await pool.totalBurned(), U('0.2'));

  const proof = await claimProofForAccount({
    settlement: matured,
    account: providerAccount,
    pool,
  });
  assert.equal(proof.claimable, true);
  const claimIntent = buildTapClaimCalldata({
    rootEpoch: proof.epoch,
    account: proof.account,
    cumulativeWei: proof.cumulative_wei,
    proof: proof.proof,
    pool: poolAddr,
    token: tokenAddr,
    chainId: 61_000,
  });
  assert.equal(claimIntent.server_signs, false);
  await (await providerA.sendTransaction(claimIntent.transaction)).wait();

  const expectedClaim = providerShareWei(auToTapWei(usdAu(2), TAP_USD_AU));
  assert.equal(await token.balanceOf(providerAccount), expectedClaim);
  const afterClaim = await claimProofForAccount({
    settlement: matured,
    account: providerAccount,
    pool,
  });
  assert.equal(afterClaim.claimable, false);
  assert.equal(afterClaim.claimable_wei, '0');
});
