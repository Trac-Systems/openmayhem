import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildAdminCommand,
  depositStateMatches,
  matchPendingTransfers,
  pendingEntriesFromState,
  pubKeyHexToMsbAddress,
  transferFromTxDetails,
  waitForDepositState,
} from '../scripts/tnk-deposit-watcher.mjs';

const pubkey = '00'.repeat(32);
const sender = pubKeyHexToMsbAddress(pubkey, 'testtrac');
const treasury = 'testtrac1treasury';

test('tnk deposit watcher matches quoted pending intents by user-derived MSB sender, treasury, and amount', () => {
  const pendingEntries = pendingEntriesFromState({
    values: [
      {
        key: 'dep/pending/memo-a',
        value: {
          memo_hash: 'memo-a',
          user: pubkey,
          status: 'pending',
          treasury_address: treasury,
          tnk_e18: '1000000000000000000',
          quoted_mu: 50_000,
          rate_tnk_usd_e6: 50_000,
          rate_source: 'gate-spot',
        },
      },
    ],
  });
  const transfers = [
    {
      hash: 'a'.repeat(64),
      from: sender,
      to: treasury,
      tnk_e18: '1000000000000000000',
      confirmed_length: 42,
    },
  ];

  const { matches, skipped } = matchPendingTransfers({
    pendingEntries,
    transfers,
    treasuryAddress: treasury,
    addressPrefix: 'testtrac',
  });

  assert.equal(skipped.length, 0);
  assert.deepEqual(matches, [
    {
      memo_hash: 'memo-a',
      user: pubkey,
      from: sender,
      treasury_address: treasury,
      tnk_e18: '1000000000000000000',
      msb_tx_hash: 'a'.repeat(64),
      confirmed_length: 42,
      quoted_mu: 50_000,
      rate_tnk_usd_e6: 50_000,
      rate_source: 'gate-spot',
    },
  ]);
});

test('tnk deposit watcher skips unquoted intents because canonical intents must be quoted', () => {
  const pendingEntries = pendingEntriesFromState([
    {
      key: 'dep/pending/memo-unquoted',
      value: {
        memo_hash: 'memo-unquoted',
        user: pubkey,
        status: 'pending',
      },
    },
  ]);
  const transfers = [
    {
      hash: 'b'.repeat(64),
      from: sender,
      to: treasury,
      tnk_e18: '2000000000000000000',
      confirmed_length: 43,
    },
  ];

  const strict = matchPendingTransfers({
    pendingEntries,
    transfers,
    treasuryAddress: treasury,
    addressPrefix: 'testtrac',
  });
  assert.equal(strict.matches.length, 0);
  assert.match(strict.skipped[0].reason, /no quoted TNK amount/i);

  const amountOnly = matchPendingTransfers({
    pendingEntries: pendingEntriesFromState([
      {
        key: 'dep/pending/memo-amount-only',
        value: {
          memo_hash: 'memo-amount-only',
          user: pubkey,
          status: 'pending',
          treasury_address: treasury,
          tnk_e18: '2000000000000000000',
        },
      },
    ]),
    transfers,
    treasuryAddress: treasury,
    addressPrefix: 'testtrac',
  });
  assert.equal(amountOnly.matches.length, 0);
  assert.match(amountOnly.skipped[0].reason, /no quoted mu_usd/i);
});

test('tnk deposit watcher normalizes MSB transfer details and redacts password from copy/paste command', () => {
  const transfer = transferFromTxDetails(
    { hash: 'c'.repeat(64), confirmed_length: 44 },
    {
      type: 12,
      address: sender,
      tro: {
        to: treasury,
        am: '3000000000000000000',
      },
    }
  );
  assert.deepEqual(transfer, {
    hash: 'c'.repeat(64),
    confirmed_length: 44,
    from: sender,
    to: treasury,
    tnk_e18: '3000000000000000000',
  });

  const command = buildAdminCommand(
    {
      memo_hash: 'memo-c',
      tnk_e18: transfer.tnk_e18,
      msb_tx_hash: transfer.hash,
    },
    {
      mayhemBin: 'mayhem',
      epoch: 7,
      at: 3600,
      submit: true,
      json: true,
      rpcUrl: 'http://127.0.0.1:49223/v1',
      walletPassword: 'secret-password',
    }
  );
  assert.match(command, /mayhem/);
  assert.match(command, /--submit/);
  assert.match(command, /'--rpc-url' 'http:\/\/127\.0\.0\.1:49223\/v1'/);
  assert.doesNotMatch(command, /secret-password/);
  assert.doesNotMatch(command, /wallet-password/);
});

test('tnk deposit watcher verifies pending removal, balance credit, and deposit root', async () => {
  const match = {
    memo_hash: 'memo-a',
    user: pubkey,
    quoted_mu: 50_000,
  };
  const state = {
    pending: null,
    balance: {
      user: pubkey,
      denom: 'mu_usd',
      mu: 50_000,
    },
    depositRoot: {
      type: 'deposit_root',
      epoch: 7,
      count: 1,
      mu_total: 50_000,
    },
  };
  assert.equal(depositStateMatches(match, { ...state, epoch: 7 }), true);
  assert.equal(depositStateMatches(match, {
    ...state,
    pending: { status: 'pending' },
    epoch: 7,
  }), false);
  assert.equal(depositStateMatches(match, {
    ...state,
    balance: { ...state.balance, mu: 49_999 },
    epoch: 7,
  }), false);

  const seenKeys = [];
  const result = await waitForDepositState(match, {
    rpcUrl: 'http://127.0.0.1:49223/v1',
    epoch: 7,
    fetchImpl: async (url) => {
      const raw = String(url);
      seenKeys.push(new URL(raw).searchParams.get('key'));
      if (raw.includes('dep%2Fpending%2Fmemo-a')) {
        return { ok: true, status: 200, json: async () => ({ value: null }) };
      }
      if (raw.includes('bal%2F')) {
        return { ok: true, status: 200, json: async () => ({ value: state.balance }) };
      }
      return { ok: true, status: 200, json: async () => ({ value: state.depositRoot }) };
    },
  });

  assert.equal(result.verified, true);
  assert.deepEqual(seenKeys.sort(), [
    `bal/${pubkey}`,
    'dep/pending/memo-a',
    'ev/dep/7',
  ].sort());
});
