import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { deployPool } from '../scripts/deploy-local.mjs';
import {
  adminSubmitFailureIsTransient,
  buildAdminCommand,
  buildAdminCommandArgs,
  buildAdminReversalCommand,
  buildAdminReversalCommandArgs,
  mergeDeposits,
  reconcileCreditedDeposits,
  redactRpcUrl,
  resolveActiveBillingEpoch,
  runAdminCommandWithRetry,
  scanTapDeposits,
  TAP_DEPOSIT_EVENT_SIGNATURE,
  TAP_DEPOSIT_WATCHER_ID,
  tapDepositKey,
  tapDepositReversalFromCredit,
  tapDepositReversalKey,
  tapDepositReversalStateMatches,
  tapDepositStateMatches,
  tapWeiToAu,
} from '../scripts/tap-deposit-watcher.mjs';

const CHAIN_ID = 61000;
const TAP_USD_AU = '2000000000000000000';
const U = (n) => ethers.parseUnits(String(n), 18);

test('tap deposit watcher redacts private RPC credentials and paths in reports', () => {
  assert.equal(
    redactRpcUrl('https://user:password@rpc.example/private-api-key?token=secret'),
    'https://***:***@rpc.example/...',
  );
  assert.equal(redactRpcUrl('not a URL'), '<redacted>');
});

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
  const canonicalWho = 'c'.repeat(64);
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
  assert.equal(
    tapDepositKey(deposit),
    `${CHAIN_ID}/${deposit.pool_address}/${deposit.eth_tx_hash}/0/${deposit.block_hash}`
  );
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: { ...deposit, who: canonicalWho, ethereum_address: deposit.who, epoch: 9 },
    balance: { user: canonicalWho, rail: 'tap', denom: 'au_usd', au: '2000000000000000000' },
    depositRoot: { type: 'deposit_root', epoch: 9, count: 1, au_total: '2000000000000000000' },
  }), true);
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: { ...deposit, who: canonicalWho, ethereum_address: deposit.who, epoch: 9 },
    balance: { user: canonicalWho, rail: 'tap', denom: 'au_usd', au: '0' },
    depositRoot: { type: 'deposit_root', epoch: 9, count: 1, au_total: '2000000000000000000' },
  }), true, 'later legitimate spending does not invalidate the immutable credit marker');
  assert.equal(tapDepositStateMatches(deposit, {
    epoch: 9,
    seen: null,
    balance: { user: canonicalWho, rail: 'tap', denom: 'au_usd', au: '2000000000000000000' },
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

test('tap deposit watcher keeps a 12-block gap behind the finalized reference', async () => {
  const queries = [];
  const provider = {
    async send(method, args) {
      assert.equal(method, 'eth_getBlockByNumber');
      assert.deepEqual(args, ['finalized', false]);
      return { number: '0x64' };
    },
  };
  const poolAddress = '0x2222222222222222222222222222222222222222';
  const pool = {
    runner: { provider },
    filters: { Deposit: () => 'deposit-filter' },
    async queryFilter(_filter, from, to) {
      queries.push([from, to]);
      return [{
        args: { buyer: '0x1111111111111111111111111111111111111111', amount: U(1) },
        index: 0,
        blockNumber: 88,
        blockHash: `0x${'b'.repeat(64)}`,
        transactionHash: `0x${'a'.repeat(64)}`,
        address: poolAddress,
      }];
    },
  };

  const scan = await scanTapDeposits({
    pool,
    fromBlock: 0,
    blockTag: 'finalized',
    chainId: CHAIN_ID,
    poolAddress,
  });
  assert.deepEqual(queries, [[0, 88]]);
  assert.equal(scan.to, 88);
  assert.equal(scan.deposits[0].confirmation_depth, 12);
  assert.equal(scan.deposits[0].confirmation_policy, 'finalized-tag');

  await assert.rejects(
    () => scanTapDeposits({
      pool,
      fromBlock: 0,
      toBlock: 89,
      blockTag: 'finalized',
      chainId: CHAIN_ID,
      poolAddress,
    }),
    /12 blocks behind the finalized reference/
  );
});

test('tap deposit watcher reverses a missing block identity before accepting its replacement', () => {
  const credited = {
    who: '0x1111111111111111111111111111111111111111',
    tap_wei: U(1).toString(),
    eth_tx_hash: `0x${'a'.repeat(64)}`,
    log_index: 0,
    block_number: 123,
    block_hash: `0x${'b'.repeat(64)}`,
    pool_address: '0x2222222222222222222222222222222222222222',
    chain_id: 1,
    finalized_block_number: 135,
    confirmation_depth: 12,
    confirmation_policy: 'finalized-tag',
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: TAP_DEPOSIT_WATCHER_ID,
  };
  const replacement = {
    ...credited,
    block_hash: `0x${'c'.repeat(64)}`,
  };
  const scan = {
    from: 100,
    to: 123,
    referenceBlock: 135,
    finalizedPolicy: true,
  };

  const reversals = reconcileCreditedDeposits([credited], [replacement], scan);
  assert.equal(reversals.length, 1);
  assert.equal(reversals[0].block_hash, credited.block_hash);
  assert.equal(reversals[0].reason, 'canonical_event_missing');
  assert.equal(tapDepositReversalKey(reversals[0]), `${tapDepositKey(credited)}/reversal`);
  assert.notEqual(tapDepositKey(replacement), tapDepositKey(credited));

  const args = buildAdminReversalCommandArgs(reversals[0], {
    epoch: 9,
    at: 32_400,
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.equal(args.includes('tap-deposit-reversal'), true);
  assert.equal(args.includes('--reconciliation-from-block'), true);
  assert.equal(args.includes('--finalized-block-number'), true);
  assert.equal(args.includes('secret'), true);
  const command = buildAdminReversalCommand(reversals[0], {
    epoch: 9,
    at: 32_400,
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.doesNotMatch(command, /secret|wallet-password/);
  assert.equal(tapDepositReversalStateMatches(reversals[0], {
    epoch: 9,
    seen: { ...credited, reversed: true },
    reversalSeen: reversals[0],
    depositRoot: { type: 'deposit_root', epoch: 9 },
  }), true);
});

test('tap deposit watcher requires finalized scans on mainnet and bounds its rescan window', async () => {
  const queries = [];
  const provider = {
    async send(method, args) {
      assert.equal(method, 'eth_getBlockByNumber');
      assert.deepEqual(args, ['finalized', false]);
      return { number: '0xc8' };
    },
  };
  const pool = {
    runner: { provider },
    filters: { Deposit: () => 'deposit-filter' },
    async queryFilter(_filter, from, to) {
      queries.push([from, to]);
      return [];
    },
  };
  const scan = await scanTapDeposits({
    pool,
    fromBlock: 100,
    chainId: 1,
    poolAddress: '0x2222222222222222222222222222222222222222',
    lookbackBlocks: 10,
  });
  assert.deepEqual(queries, [[91, 188]]);
  assert.equal(scan.finalizedPolicy, true);
  assert.equal(scan.referenceBlock, 200);

  await assert.rejects(
    scanTapDeposits({
      pool,
      fromBlock: 100,
      blockTag: 'latest',
      chainId: 1,
      poolAddress: '0x2222222222222222222222222222222222222222',
    }),
    /mainnet.*requires.*finalized/i,
  );
});

test('tap deposit watcher retries broad transient failures but stops on permanent rejection', async () => {
  const children = [
    { status: 1, stdout: '', stderr: 'HTTP 503 temporarily unavailable' },
    { status: 1, stdout: '', stderr: 'ECONNRESET' },
    { status: 0, stdout: '{}', stderr: '' },
  ];
  const delays = [];
  const retried = await runAdminCommandWithRetry({
    mayhemBin: 'mayhem',
    buildArgs: () => ['admin'],
    attempts: 5,
    baseDelayMs: 10,
    maxDelayMs: 100,
    spawnImpl: () => children.shift(),
    sleep: async (delay) => delays.push(delay),
  });
  assert.equal(retried.child.status, 0);
  assert.equal(retried.attempts, 3);
  assert.deepEqual(delays, [10, 20]);
  assert.equal(adminSubmitFailureIsTransient({ status: 1, stderr: 'socket hang up' }), true);

  let permanentAttempts = 0;
  const permanent = await runAdminCommandWithRetry({
    mayhemBin: 'mayhem',
    buildArgs: () => ['admin'],
    spawnImpl: () => {
      permanentAttempts += 1;
      return { status: 2, stdout: '', stderr: 'invalid signed feature payload' };
    },
    sleep: async () => assert.fail('permanent failure must not sleep'),
  });
  assert.equal(permanent.attempts, 1);
  assert.equal(permanent.transient, false);
  assert.equal(permanentAttempts, 1);
});
