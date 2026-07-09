import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import {
  buildTapClaimCalldata,
  buildTapDepositCalldata,
  executeTapDeposit,
  tapAmountToWei,
} from '../scripts/tap-calldata-builder.mjs';
import { claimProofForAccount } from '../scripts/tap-claim-proof.mjs';
import { deployPool } from '../scripts/deploy-local.mjs';
import {
  auToTapWei,
  providerShareWei,
  rollTapSettlement,
} from '../scripts/tap-settlement-roller.mjs';
import { makeReceiptIdentity, signedTapReceipt } from './helpers/signed-receipt.mjs';

const TAP_USD_AU = '1000000000000000000';
const usdAu = (value) => (BigInt(value) * 1_000_000_000_000_000_000n).toString();
const U = (n) => ethers.parseUnits(String(n), 18);
const testKey = (value) => `0x${value.toString(16).padStart(64, '0')}`;
const OPERATOR_KEY = testKey(1);
const BUYER_KEY = testKey(2);
const POOR_BUYER_KEY = testKey(3);

const receipt = signedTapReceipt;

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

async function localPoolWithKnownKeys({ poorBuyerEth = '0' } = {}) {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: {
      accounts: [
        { secretKey: OPERATOR_KEY, balance: ethers.toBeHex(ethers.parseEther('100')) },
        { secretKey: BUYER_KEY, balance: ethers.toBeHex(ethers.parseEther('100')) },
        { secretKey: POOR_BUYER_KEY, balance: ethers.toBeHex(ethers.parseEther(poorBuyerEth)) },
      ],
    },
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

test('local wallet TAP deposit dry-run simulates without moving funds', async () => {
  const { provider, token, pool, tokenAddr, poolAddr } = await localPoolWithKnownKeys();
  const buyer = new ethers.Wallet(BUYER_KEY, provider);
  await (await token.mint(await buyer.getAddress(), U('2'))).wait();

  const report = await executeTapDeposit({
    privateKey: BUYER_KEY,
    provider,
    amountTap: '1.25',
    token: tokenAddr,
    pool: poolAddr,
    chainId: 61_000,
    confirm: false,
  });

  assert.equal(report.custody, 'local_wallet');
  assert.equal(report.server_signs, false);
  assert.equal(report.submitted, false);
  assert.equal(report.from, await buyer.getAddress());
  assert.equal(report.simulation.approve.static_call_ok, true);
  assert.equal(report.simulation.approve.gas_estimate_ok, true);
  assert.equal(report.gas_precheck.ok, true);
  assert.equal(report.copy_paste_replay_command, undefined);
  assert.equal(await pool.totalDeposited(), 0n);
  assert.equal(await token.allowance(await buyer.getAddress(), poolAddr), 0n);
});

test('local wallet TAP deposit confirm signs approve then deposit', async () => {
  const { provider, token, pool, tokenAddr, poolAddr } = await localPoolWithKnownKeys();
  const buyer = new ethers.Wallet(BUYER_KEY, provider);
  await (await token.mint(await buyer.getAddress(), U('2'))).wait();

  const report = await executeTapDeposit({
    privateKey: BUYER_KEY,
    provider,
    amountTap: '1.25',
    token: tokenAddr,
    pool: poolAddr,
    chainId: 61_000,
    confirm: true,
  });

  assert.equal(report.submitted, true);
  assert.equal(report.transactions.length, 2);
  assert.equal(report.transactions[0].step, 'approve');
  assert.equal(report.transactions[0].status, 1);
  assert.equal(report.transactions[1].step, 'deposit');
  assert.equal(report.transactions[1].status, 1);
  assert.equal(report.simulation.deposit.static_call_ok, true);
  assert.equal(report.simulation.deposit.gas_estimate_ok, true);
  assert.equal(await pool.totalDeposited(), tapAmountToWei('1.25'));
});

test('local wallet TAP deposit refuses insufficient gas before approval', async () => {
  const { provider, token, pool, tokenAddr, poolAddr } = await localPoolWithKnownKeys({ poorBuyerEth: '0.000000000000000001' });
  const poorBuyer = new ethers.Wallet(POOR_BUYER_KEY, provider);
  const poorBuyerAddress = await poorBuyer.getAddress();
  await (await token.mint(poorBuyerAddress, U('2'))).wait();

  await assert.rejects(
    () => executeTapDeposit({
      privateKey: POOR_BUYER_KEY,
      provider,
      amountTap: '1.25',
      token: tokenAddr,
      pool: poolAddr,
      chainId: 61_000,
      confirm: true,
    }),
    /not enough ETH for gas|insufficient funds/i,
  );
  assert.equal(await token.allowance(poorBuyerAddress, poolAddr), 0n);
  assert.equal(await pool.totalDeposited(), 0n);
});

test('claim calldata executes MayhemInferencePool.claim from provider wallet', async () => {
  const { provider, operator, token, pool, poolAddr } = await localPool();
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const providerAccount = await providerA.getAddress();

  await (await token.mint(await buyer.getAddress(), U('3'))).wait();
  await (await token.connect(buyer).approve(poolAddr, U('3'))).wait();
  await (await pool.connect(buyer).deposit(U('3'))).wait();

  const providerId = makeReceiptIdentity();
  const rolled = await rollTapSettlement({
    bundle: {
      epoch: 1,
      receipts: [receipt({ session: 's1', provider: providerId, au: usdAu(2) })],
    },
    providerAccounts: { [providerId.publicKeyHex]: providerAccount },
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
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

  const expectedClaim = providerShareWei(auToTapWei(usdAu(2), TAP_USD_AU));
  assert.equal(await token.balanceOf(providerAccount), expectedClaim);
});
