import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import {
  buildAdminCommand,
  buildAdminCommandArgs,
  scanTapDeposits,
  TAP_DEPOSIT_EVENT_SIGNATURE,
  TAP_DEPOSIT_WATCHER_ID,
  tapDepositStateMatches,
  tapWeiToMu,
} from '../scripts/tap-deposit-watcher.mjs';

const CHAIN_ID = 61000;
const U = (n) => ethers.parseUnits(String(n), 18);

test('tap deposit watcher scans KnowledgePool Deposit logs and builds admin command', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: CHAIN_ID },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const { token, pool, poolAddr } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(2))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(2))).wait();
  await (await pool.connect(buyer).deposit(U(2))).wait();

  const latest = Number(await provider.send('eth_blockNumber', []));
  const scan = await scanTapDeposits({
    pool,
    fromBlock: 0,
    toBlock: latest,
    tapUsdE6: 2_000_000,
    chainId: CHAIN_ID,
    poolAddress: poolAddr,
  });

  assert.equal(scan.deposits.length, 1);
  const deposit = scan.deposits[0];
  assert.equal(deposit.who, (await buyer.getAddress()).toLowerCase());
  assert.equal(deposit.tap_wei, U(2).toString());
  assert.equal(deposit.tap_usd_e6, 2_000_000);
  assert.equal(deposit.mu, 4_000_000);
  assert.equal(deposit.pool_address, poolAddr.toLowerCase());
  assert.equal(deposit.chain_id, CHAIN_ID);
  assert.match(deposit.eth_tx_hash, /^0x[0-9a-f]{64}$/);
  assert.match(deposit.block_hash, /^0x[0-9a-f]{64}$/);
  assert.equal(deposit.log_index >= 0, true);
  assert.equal(deposit.finalized_block_number >= deposit.block_number, true);
  assert.equal(deposit.confirmation_depth, deposit.finalized_block_number - deposit.block_number);
  assert.equal(deposit.confirmation_policy, 'explicit-to-block');
  assert.equal(deposit.event_signature, TAP_DEPOSIT_EVENT_SIGNATURE);
  assert.equal(deposit.watcher_id, TAP_DEPOSIT_WATCHER_ID);

  const args = buildAdminCommandArgs(deposit, {
    epoch: 7,
    at: 25_200,
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.deepEqual(args.slice(0, 4), ['admin', 'tap-deposit', '--who', deposit.who]);
  assert.equal(args.includes('--wallet-password'), true);
  assert.equal(args.includes('secret'), true);
  assert.equal(args.includes('--tap-usd-e6'), false);
  assert.equal(args.includes('--block-hash'), true);
  assert.equal(args.includes('--finalized-block-number'), true);
  assert.equal(args.includes('--confirmation-depth'), true);
  assert.equal(args.includes('--confirmation-policy'), true);
  assert.equal(args.includes('--event-signature'), true);
  assert.equal(args.includes('--watcher-id'), true);

  const command = buildAdminCommand(deposit, {
    epoch: 7,
    at: 25_200,
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.match(command, /tap-deposit/);
  assert.match(command, /--eth-tx-hash/);
  assert.match(command, /--block-hash/);
  assert.match(command, /--event-signature/);
  assert.doesNotMatch(command, /tap-usd-e6/);
  assert.doesNotMatch(command, /secret/);
  assert.doesNotMatch(command, /wallet-password/);
});

test('tap deposit watcher helpers compute policy mu and verify replay-safe state shape', () => {
  const deposit = {
    who: '0x1111111111111111111111111111111111111111',
    tap_wei: U(1).toString(),
    tap_usd_e6: 2_000_000,
    mu: 2_000_000,
    eth_tx_hash: `0x${'a'.repeat(64)}`,
    log_index: 0,
    block_number: 123,
    block_hash: `0x${'b'.repeat(64)}`,
    pool_address: '0x2222222222222222222222222222222222222222',
    chain_id: CHAIN_ID,
    finalized_block_number: 135,
    confirmation_depth: 12,
    confirmation_policy: 'depth-12',
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: TAP_DEPOSIT_WATCHER_ID,
  };

  assert.equal(tapWeiToMu(deposit.tap_wei, deposit.tap_usd_e6), 2_000_000);
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: { ...deposit },
    balance: { user: deposit.who, denom: 'mu_usd', mu: 2_000_000 },
    depositRoot: { type: 'deposit_root', epoch: 9, count: 1, mu_total: 2_000_000 },
  }), true);
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: null,
    balance: { user: deposit.who, denom: 'mu_usd', mu: 2_000_000 },
    depositRoot: { type: 'deposit_root', epoch: 9, count: 1, mu_total: 2_000_000 },
  }), false);
});

test('tap deposit watcher emits no credit for nonexistent Deposit logs', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: CHAIN_ID },
    wallet: { totalAccounts: 2 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const { pool, poolAddr } = await deployPool(operator);

  const latest = Number(await provider.send('eth_blockNumber', []));
  const scan = await scanTapDeposits({
    pool,
    fromBlock: 0,
    toBlock: latest,
    tapUsdE6: 2_000_000,
    chainId: CHAIN_ID,
    poolAddress: poolAddr,
  });

  assert.deepEqual(scan.deposits, []);
});
