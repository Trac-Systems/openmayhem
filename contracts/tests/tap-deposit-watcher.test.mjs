import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import {
  buildAdminCommand,
  buildAdminCommandArgs,
  mergeDeposits,
  resolveActiveBillingEpoch,
  scanTapDeposits,
  TAP_DEPOSIT_EVENT_SIGNATURE,
  TAP_DEPOSIT_WATCHER_ID,
  tapDepositStateMatches,
  tapWeiToAu,
} from '../scripts/tap-deposit-watcher.mjs';

const CHAIN_ID = 61000;
const TAP_USD_AU = '2000000000000000000';
const U = (n) => ethers.parseUnits(String(n), 18);

test('tap deposit watcher derives the active billing epoch from ledger apply state', async () => {
  const stateFetch = async () => ({
    ok: true,
    json: async () => ({ value: { updated_epoch: 7 } }),
  });
  const emptyFetch = async () => ({ ok: true, json: async () => ({ value: null }) });

  assert.equal(await resolveActiveBillingEpoch(undefined, 'http://peer/v1', { fetchImpl: stateFetch }), 8);
  assert.equal(await resolveActiveBillingEpoch(undefined, 'http://peer/v1', { fetchImpl: emptyFetch }), 1);
  assert.equal(await resolveActiveBillingEpoch('12', null), 12);
  await assert.rejects(
    resolveActiveBillingEpoch(undefined, null),
    /Missing --epoch or --peer-rpc/,
  );
});

test('tap deposit watcher scans MayhemInferencePool Deposit logs and builds admin command', async () => {
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
  for (let i = 0; i < 12; i++) {
    await provider.send('evm_mine', []);
  }

  const scan = await scanTapDeposits({
    pool,
    fromBlock: 0,
    tapUsdAu: TAP_USD_AU,
    chainId: CHAIN_ID,
    poolAddress: poolAddr,
  });

  assert.equal(scan.deposits.length, 1);
  const deposit = scan.deposits[0];
  assert.equal(deposit.who, (await buyer.getAddress()).toLowerCase());
  assert.equal(deposit.tap_wei, U(2).toString());
  assert.equal(deposit.tap_usd_au, TAP_USD_AU);
  assert.equal(deposit.au, '4000000000000000000');
  assert.equal(deposit.pool_address, poolAddr.toLowerCase());
  assert.equal(deposit.chain_id, CHAIN_ID);
  assert.match(deposit.eth_tx_hash, /^0x[0-9a-f]{64}$/);
  assert.match(deposit.block_hash, /^0x[0-9a-f]{64}$/);
  assert.equal(deposit.log_index >= 0, true);
  assert.equal(deposit.finalized_block_number >= deposit.block_number, true);
  assert.equal(deposit.confirmation_depth, deposit.finalized_block_number - deposit.block_number);
  assert.equal(deposit.confirmation_policy, `depth-${deposit.confirmation_depth}`);
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
  assert.equal(args.includes('--tap-usd-au'), false);
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
  assert.doesNotMatch(command, /tap-usd-au/);
  assert.doesNotMatch(command, /secret/);
  assert.doesNotMatch(command, /wallet-password/);
});

test('tap deposit watcher helpers compute policy au and verify replay-safe state shape', () => {
  const deposit = {
    who: '0x1111111111111111111111111111111111111111',
    tap_wei: U(1).toString(),
    tap_usd_au: TAP_USD_AU,
    au: '2000000000000000000',
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

  assert.equal(tapWeiToAu(deposit.tap_wei, deposit.tap_usd_au), '2000000000000000000');
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: { ...deposit },
    balance: { user: deposit.who, rail: 'tap', denom: 'au_usd', au: '2000000000000000000' },
    depositRoot: { type: 'deposit_root', epoch: 9, count: 1, au_total: '2000000000000000000' },
  }), true);
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: null,
    balance: { user: deposit.who, rail: 'tap', denom: 'au_usd', au: '2000000000000000000' },
    depositRoot: { type: 'deposit_root', epoch: 9, count: 1, au_total: '2000000000000000000' },
  }), false);
});

test('tap deposit watcher keeps every unsubmitted deposit past the old 1000 backlog', () => {
  const mkDeposit = (idx, overrides = {}) => ({
    who: '0x1111111111111111111111111111111111111111',
    tap_wei: U(1).toString(),
    eth_tx_hash: `0x${idx.toString(16).padStart(64, '0')}`,
    log_index: idx % 3,
    block_number: idx,
    block_hash: `0x${(idx + 1).toString(16).padStart(64, '0')}`,
    pool_address: '0x2222222222222222222222222222222222222222',
    chain_id: CHAIN_ID,
    finalized_block_number: idx + 12,
    confirmation_depth: 12,
    confirmation_policy: 'depth-12',
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: TAP_DEPOSIT_WATCHER_ID,
    ...overrides,
  });
  const backlog = Array.from({ length: 1_205 }, (_, idx) => mkDeposit(idx + 1));
  const replacement = mkDeposit(17, { block_number: 2_000 });

  const merged = mergeDeposits(backlog, [replacement]);

  assert.equal(merged.length, 1_205);
  assert.equal(merged[0].block_number, 1);
  assert.equal(merged.at(-1).block_number, 2_000);
  assert.equal(
    merged.filter((deposit) => deposit.eth_tx_hash === replacement.eth_tx_hash).length,
    1
  );
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

  const scan = await scanTapDeposits({
    pool,
    fromBlock: 0,
    tapUsdAu: TAP_USD_AU,
    chainId: CHAIN_ID,
    poolAddress: poolAddr,
  });

  assert.deepEqual(scan.deposits, []);
});

test('tap deposit watcher refuses unsafe shallow depth scans', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: CHAIN_ID },
    wallet: { totalAccounts: 2 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const { pool, poolAddr } = await deployPool(operator);
  const latest = Number(await provider.send('eth_blockNumber', []));

  await assert.rejects(
    () => scanTapDeposits({
      pool,
      fromBlock: 0,
      confirmations: 0,
      tapUsdAu: TAP_USD_AU,
      chainId: CHAIN_ID,
      poolAddress: poolAddr,
    }),
    /confirmations must be at least 12/
  );
  await assert.rejects(
    () => scanTapDeposits({
      pool,
      fromBlock: 0,
      toBlock: latest,
      tapUsdAu: TAP_USD_AU,
      chainId: CHAIN_ID,
      poolAddress: poolAddr,
    }),
    /to-block must be at least 12 confirmations deep/
  );
});
