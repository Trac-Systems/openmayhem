import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  depositFeatureKey,
  execute,
  executeDepositFeature,
  executeRateFeature,
  executeTapAccountBindingFeature,
  makeEthereumIdentity,
  makeIdentity,
  makeVerifier,
  rateFeatureKey,
  signConsent,
  signDepositTnkIntent,
  signTapAccountBinding,
} from './helpers/contract.js';

const rulesHash = '6'.repeat(64);
const oneTnkE18 = '1000000000000000000';
const treasury = `testtrac1${'1'.repeat(40)}`;
const TAP_DEPOSIT_EVENT_SIGNATURE = '0xe1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c';
const TAP_DEPOSIT_WATCHER_ID = 'tap-deposit-watcher-v1';

const providerRegistration = {
  op: 'register_provider',
};

async function setupRateContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  for (const op of [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
      txNo: 1,
    },
    {
      type: 'setPayments',
      value: {
        op: 'set_payments',
        ver: 1,
        fiat: { processor: 'stripe', currencies: ['usd', 'eur'], locale: 'en' },
        tap: {
          chain_id: 61_000,
          token_address: `0x${'1'.repeat(40)}`,
          pool_address: `0x${'2'.repeat(40)}`,
        },
        tnk: { network: 'testnet1', treasury_address: treasury },
      },
      sender: admin.publicKey,
      txNo: 2,
    },
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider.wallet, 1, rulesHash),
      },
      sender: provider.publicKey,
      txNo: 3,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { admin, provider, user, outsider, storage, contract };
}

const rateOracle = (overrides = {}) => ({
  op: 'rate_oracle',
  tnk_usd_au: '2000000000000000000',
  source: 'gate-spot',
  ts: 1_000,
  ...overrides,
});

const tapRateOracle = (overrides = {}) => ({
  op: 'tap_rate_oracle',
  tap_usd_au: '2000000000000000000',
  source: 'uniswap-v2',
  ts: 1_000,
  ...overrides,
});

async function tnkDepositIntent(contract, storage, user, memoHash, overrides = {}) {
  const intent = {
    op: 'deposit_tnk',
    memo_hash: memoHash,
    treasury_address: treasury,
    tnk_e18: oneTnkE18,
    quoted_au: '2000000000000000000',
    rate_tnk_usd_au: '2000000000000000000',
    rate_source: 'gate-spot',
    ...overrides,
  };
  return await executeDepositFeature(
    contract,
    storage,
    {
      op: 'deposit_tnk',
      sender: user.publicKey,
      intent,
      sig: signDepositTnkIntent(user.wallet, intent),
    },
    user.publicKey
  );
}

const tnkDepositConfirm = (contract, user, memoHash, overrides = {}) => ({
  op: 'tnk_deposit',
  memo_hash: memoHash,
  msb_transfer: {
    schema_version: 1,
    network: 'testnet1',
    tx_hash: 'd'.repeat(64),
    confirmed_length: 100,
    observed_signed_length: 112,
    from: contract.msbAddressForPublicKey(user.publicKey, 'testnet1'),
    to: treasury,
    amount_e18: oneTnkE18,
    ...(overrides.msb_transfer ?? {}),
  },
  epoch: 1,
  at: 1_900,
  ...Object.fromEntries(Object.entries(overrides).filter(([key]) => key !== 'msb_transfer')),
});

test('MayhemContract rateOracle feature is admin controlled and monotonic', async () => {
  const { admin, outsider, storage, contract } = await setupRateContract();

  const paidTxRoute = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle(),
    admin.publicKey,
    4
  );
  assert.match(paidTxRoute.message, /unknown contract operation type|function not registered/i);

  const nonAdmin = await executeRateFeature(
    contract,
    storage,
    rateOracle(),
    outsider.publicKey,
  );
  assert.match(nonAdmin.message, /admin required/i);

  const badSource = await executeRateFeature(
    contract,
    storage,
    rateOracle({ source: 'provider-quote' }),
    admin.publicKey,
  );
  assert.match(badSource.message, /unsupported rate source/i);

  const firstValue = rateOracle();
  const firstKey = await rateFeatureKey(contract, firstValue);
  const first = await executeRateFeature(contract, storage, firstValue, admin.publicKey);
  assert.deepEqual(first, {
    ok: true,
    op: 'rateOracle',
    ts: 1_000,
    source: 'gate-spot',
  });
  assert.deepEqual((await storage.get('rate/latest')).value, {
    denom: 'tnk_usd_au',
    tnk_usd_au: '2000000000000000000',
    source: 'gate-spot',
    ts: 1_000,
    updated_at: firstKey,
    posted_by: admin.publicKey,
    posted_by_role: 'admin',
  });
  assert.deepEqual((await storage.get(firstKey)).value, (await storage.get('rate/latest')).value);

  const older = await executeRateFeature(
    contract,
    storage,
    rateOracle({ source: 'mexc-spot', ts: 999 }),
    admin.publicKey,
  );
  assert.match(older.message, /timestamp must not decrease/i);
});

test('MayhemContract tapRateOracle drives TAP deposits and fails closed when stale', async () => {
  const { admin, user, outsider, storage, contract } = await setupRateContract();
  const ethereum = makeEthereumIdentity();
  const buyer = ethereum.address;
  const pool = '0x4444444444444444444444444444444444444444';
  const tapDeposit = {
    op: 'tap_deposit',
    who: buyer,
    tap_wei: oneTnkE18,
    eth_tx_hash: `0x${'b'.repeat(64)}`,
    log_index: 0,
    block_number: 123,
    block_hash: `0x${'c'.repeat(64)}`,
    pool_address: pool,
    chain_id: 61_000,
    finalized_block_number: 135,
    confirmation_depth: 12,
    confirmation_policy: 'depth-12',
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: TAP_DEPOSIT_WATCHER_ID,
    epoch: 1,
    at: 3_701,
  };

  const paidTxRoute = await execute(
    contract,
    storage,
    'tapRateOracle',
    tapRateOracle(),
    admin.publicKey,
    4
  );
  assert.match(paidTxRoute.message, /unknown contract operation type|function not registered/i);

  const nonAdmin = await executeRateFeature(
    contract,
    storage,
    tapRateOracle(),
    outsider.publicKey,
  );
  assert.match(nonAdmin.message, /admin required/i);

  const badSource = await executeRateFeature(
    contract,
    storage,
    tapRateOracle({ source: 'provider-quote' }),
    admin.publicKey,
  );
  assert.match(badSource.message, /unsupported TAP rate source/i);

  const firstValue = tapRateOracle();
  const firstKey = await rateFeatureKey(contract, firstValue);
  const first = await executeRateFeature(contract, storage, firstValue, admin.publicKey);
  assert.deepEqual(first, {
    ok: true,
    op: 'tapRateOracle',
    ts: 1_000,
    source: 'uniswap-v2',
  });
  assert.deepEqual((await storage.get('tap/rate/latest')).value, {
    denom: 'tap_usd_au',
    tap_usd_au: '2000000000000000000',
    source: 'uniswap-v2',
    ts: 1_000,
    updated_at: firstKey,
    posted_by: admin.publicKey,
    posted_by_role: 'admin',
  });

  const older = await executeRateFeature(
    contract,
    storage,
    tapRateOracle({ source: 'config', ts: 999 }),
    admin.publicKey,
  );
  assert.match(older.message, /timestamp must not decrease/i);

  const staleDeposit = await executeDepositFeature(
    contract,
    storage,
    tapDeposit,
    admin.publicKey
  );
  assert.match(staleDeposit.message, /TAP rate oracle is stale/i);
  assert.equal(await storage.get(`bal/${buyer}/tap`), null);

  const userConsent = await execute(
    contract,
    storage,
    'consent',
    { op: 'consent', ver: 1, hash: rulesHash, sig: signConsent(user.wallet, 1, rulesHash) },
    user.publicKey,
    5
  );
  assert.equal(userConsent.ok, true, userConsent.message);
  const binding = await executeTapAccountBindingFeature(
    contract,
    storage,
    signTapAccountBinding(user.wallet, ethereum, {
      op: 'tap_account_bind',
      user: user.publicKey,
      ethereum_address: buyer,
      chain_id: 61_000,
      pool_address: pool,
    }),
    admin.publicKey
  );
  assert.equal(binding.ok, true, binding.message);

  const freshTapValue = { ...tapDeposit, at: 1_900 };
  const freshTapKey = await depositFeatureKey(contract, freshTapValue);
  const freshDeposit = await executeDepositFeature(
    contract,
    storage,
    freshTapValue,
    admin.publicKey
  );
  assert.equal(freshDeposit.ok, true, freshDeposit.message);
  assert.equal(freshDeposit.au, '2000000000000000000');
  assert.equal(freshDeposit.rate_ts, 1_000);
  assert.deepEqual((await storage.get(`bal/${user.publicKey}/tap`)).value, {
    user: user.publicKey,
    rail: 'tap',
    denom: 'au_usd',
    au: '2000000000000000000',
    updated_epoch: 1,
    updated_at: freshTapKey,
    last_deposit_rail: 'tap',
    last_deposit_rate_ts: 1_000,
    last_deposit_rate_source: 'uniswap-v2',
    last_deposit_tap_usd_au: '2000000000000000000',
  });
});

test('MayhemContract refuses TNK deposit credits when the rate is stale', async () => {
  const { admin, user, storage, contract } = await setupRateContract();
  const rate = await executeRateFeature(contract, storage, rateOracle(), admin.publicKey);
  assert.equal(rate.ok, true, rate.message);

  const staleDeposit = await executeDepositFeature(
    contract,
    storage,
    tnkDepositConfirm(contract, user, 'stale-deposit', { at: 3_701 }),
    admin.publicKey
  );
  assert.match(staleDeposit.message, /rate oracle is stale/i);
  assert.equal(await storage.get(`bal/${user.publicKey}/tnk`), null);

  const userConsent = await execute(
    contract,
    storage,
    'consent',
    {
      op: 'consent',
      ver: 1,
      hash: rulesHash,
      sig: signConsent(user.wallet, 1, rulesHash),
    },
    user.publicKey,
    7
  );
  assert.equal(userConsent.ok, true, userConsent.message);

  const intent = await tnkDepositIntent(
    contract,
    storage,
    user,
    'fresh-deposit'
  );
  assert.equal(intent.ok, true, intent.message);

  const freshTnkValue = tnkDepositConfirm(contract, user, 'fresh-deposit', {
    msb_transfer: { tx_hash: 'f'.repeat(64) },
  });
  const freshTnkKey = await depositFeatureKey(contract, freshTnkValue);
  const freshDeposit = await executeDepositFeature(
    contract,
    storage,
    freshTnkValue,
    admin.publicKey
  );
  assert.equal(freshDeposit.ok, true, freshDeposit.message);
  assert.equal(freshDeposit.op, 'tnkDeposit');
  assert.equal(freshDeposit.who, user.publicKey);
  assert.equal(freshDeposit.au, '2000000000000000000');
  assert.equal(freshDeposit.epoch, 1);
  assert.equal(freshDeposit.deposit_root.length, 64);
  assert.equal(freshDeposit.rate_ts, 1_000);
  assert.deepEqual((await storage.get(`bal/${user.publicKey}/tnk`)).value, {
    user: user.publicKey,
    rail: 'tnk',
    denom: 'au_usd',
    au: '2000000000000000000',
    updated_epoch: 0,
    updated_at: freshTnkKey,
    last_deposit_rail: 'tnk',
    last_deposit_rate_ts: 1_000,
  });
});

test('MayhemContract rate staleness follows scheduled params', async () => {
  const { admin, user, storage, contract } = await setupRateContract();
  const params = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: { rate_staleness_seconds: 60 },
    },
    admin.publicKey,
    4
  );
  assert.equal(params.ok, true, params.message);

  const rate = await executeRateFeature(
    contract,
    storage,
    rateOracle({ ts: 86_400 }),
    admin.publicKey
  );
  assert.equal(rate.ok, true, rate.message);

  const staleByScheduledParam = await executeDepositFeature(
    contract,
    storage,
    tnkDepositConfirm(contract, user, 'scheduled-stale', {
      msb_transfer: { tx_hash: '2'.repeat(64) },
      at: 86_461,
    }),
    admin.publicKey
  );
  assert.match(staleByScheduledParam.message, /rate oracle is stale/i);
});
