import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '6'.repeat(64);
const oneTnkE18 = '1000000000000000000';

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
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider.wallet, 1, rulesHash),
      },
      sender: provider.publicKey,
      txNo: 2,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 3,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { admin, provider, user, outsider, storage, contract };
}

const rateOracle = (overrides = {}) => ({
  op: 'rate_oracle',
  tnk_usd_e6: 2_000_000,
  source: 'gate-spot',
  ts: 1_000,
  ...overrides,
});

test('MayhemContract rateOracle is admin controlled and monotonic', async () => {
  const { admin, outsider, storage, contract } = await setupRateContract();

  const nonAdmin = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle(),
    outsider.publicKey,
    4
  );
  assert.match(nonAdmin.message, /admin required/i);

  const badSource = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle({ source: 'provider-quote' }),
    admin.publicKey,
    5
  );
  assert.match(badSource.message, /unsupported rate source/i);

  const first = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle(),
    admin.publicKey,
    6
  );
  assert.deepEqual(first, {
    ok: true,
    op: 'rateOracle',
    ts: 1_000,
    source: 'gate-spot',
  });
  assert.deepEqual((await storage.get('rate/latest')).value, {
    denom: 'tnk_usd_e6',
    tnk_usd_e6: 2_000_000,
    source: 'gate-spot',
    ts: 1_000,
    updated_at: makeTxKey(6),
    posted_by: admin.publicKey,
    posted_by_role: 'admin',
  });

  const older = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle({ source: 'mexc-spot', ts: 999 }),
    admin.publicKey,
    7
  );
  assert.match(older.message, /timestamp must not decrease/i);
});

test('MayhemContract refuses TNK deposits and payouts when the rate is stale', async () => {
  const { admin, provider, user, storage, contract } = await setupRateContract();
  const rate = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle(),
    admin.publicKey,
    4
  );
  assert.equal(rate.ok, true, rate.message);

  await storage.put(`earn/${provider.publicKey}`, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 2_000_000,
    held_mu: 0,
    paid_cum_mu: 0,
    updated_epoch: 0,
    updated_at: null,
  });

  const staleDeposit = await execute(
    contract,
    storage,
    'tnkDeposit',
    {
      op: 'tnk_deposit',
      memo_hash: 'stale-deposit',
      tnk_e18: oneTnkE18,
      msb_tx_hash: 'd'.repeat(64),
      epoch: 1,
      at: 3_701,
    },
    admin.publicKey,
    5
  );
  assert.match(staleDeposit.message, /rate oracle is stale/i);
  assert.equal(await storage.get(`bal/${user.publicKey}`), null);

  const staleTarget = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'trac1stalerateprovider',
      payout_method: 'tnk',
    },
    admin.publicKey,
    6
  );
  assert.equal(staleTarget.ok, true, staleTarget.message);

  const stalePayout = await execute(
    contract,
    storage,
    'payoutConfirm',
    {
      op: 'payout_confirm',
      who: provider.publicKey,
      epoch: 200,
      mu: 1_000_000,
      tnk_e18: '500000000000000000',
      msb_tx_hash: 'e'.repeat(64),
      at: 3_701,
    },
    admin.publicKey,
    7
  );
  assert.match(stalePayout.message, /rate oracle is stale/i);
  assert.equal((await storage.get(`earn/${provider.publicKey}`)).value.paid_cum_mu, 0);

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

  const intent = await execute(
    contract,
    storage,
    'depositTnk',
    {
      op: 'deposit_tnk',
      memo_hash: 'fresh-deposit',
      treasury_address: 'testtrac1treasury',
      tnk_e18: oneTnkE18,
      quoted_mu: 2_000_000,
      rate_tnk_usd_e6: 2_000_000,
      rate_source: 'gate-spot',
    },
    user.publicKey,
    8
  );
  assert.equal(intent.ok, true, intent.message);

  const freshDeposit = await execute(
    contract,
    storage,
    'tnkDeposit',
    {
      op: 'tnk_deposit',
      memo_hash: 'fresh-deposit',
      tnk_e18: oneTnkE18,
      msb_tx_hash: 'f'.repeat(64),
      epoch: 1,
      at: 1_900,
    },
    admin.publicKey,
    9
  );
  assert.equal(freshDeposit.ok, true, freshDeposit.message);
  assert.equal(freshDeposit.op, 'tnkDeposit');
  assert.equal(freshDeposit.who, user.publicKey);
  assert.equal(freshDeposit.mu, 2_000_000);
  assert.equal(freshDeposit.epoch, 1);
  assert.equal(freshDeposit.deposit_root.length, 64);
  assert.equal(freshDeposit.rate_ts, 1_000);
  assert.deepEqual((await storage.get(`bal/${user.publicKey}`)).value, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 2_000_000,
    updated_epoch: 0,
    updated_at: makeTxKey(9),
    last_deposit_rate_ts: 1_000,
  });

  const target = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'trac1providerpayouttarget',
      payout_method: 'tnk',
    },
    admin.publicKey,
    10
  );
  assert.equal(target.ok, true, target.message);

  const freshPayout = await execute(
    contract,
    storage,
    'payoutConfirm',
    {
      op: 'payout_confirm',
      who: provider.publicKey,
      epoch: 200,
      mu: 1_000_000,
      tnk_e18: '500000000000000000',
      msb_tx_hash: '1'.repeat(64),
      at: 1_900,
    },
    admin.publicKey,
    11
  );
  assert.equal(freshPayout.ok, true, freshPayout.message);
  assert.equal(freshPayout.op, 'payoutConfirm');
  assert.equal(freshPayout.kind, 'provider');
  assert.equal(freshPayout.who, provider.publicKey);
  assert.equal(freshPayout.mu, 1_000_000);
  assert.equal(freshPayout.epoch, 200);
  assert.equal(freshPayout.payout_root.length, 64);
  assert.equal(freshPayout.rate_ts, 1_000);
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 2_000_000,
    held_mu: 0,
    paid_cum_mu: 1_000_000,
    holdbacks: [],
    updated_epoch: 0,
    updated_at: makeTxKey(11),
    last_holdback_release_epoch: 200,
    last_payout_rail: 'tnk',
    last_payout_rate_ts: 1_000,
    last_payout_msb_tx_hash: '1'.repeat(64),
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

  const rate = await execute(
    contract,
    storage,
    'rateOracle',
    rateOracle({ ts: 86_400 }),
    admin.publicKey,
    5
  );
  assert.equal(rate.ok, true, rate.message);

  const staleByScheduledParam = await execute(
    contract,
    storage,
    'tnkDeposit',
    {
      op: 'tnk_deposit',
      memo_hash: 'scheduled-stale',
      tnk_e18: oneTnkE18,
      msb_tx_hash: '2'.repeat(64),
      epoch: 1,
      at: 86_461,
    },
    admin.publicKey,
    6
  );
  assert.match(staleByScheduledParam.message, /rate oracle is stale/i);
});
