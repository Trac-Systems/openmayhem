import assert from 'node:assert/strict';
import test from 'node:test';

import MayhemContract from '../contract/contract.js';
import { MemoryStorage, execute, makeIdentity, makeVerifier } from './helpers/contract.js';

const paymentConfig = (overrides = {}) => ({
  op: 'set_payments',
  ver: 1,
  fiat: {
    processor: 'stripe',
    integration_currency: 'usd',
    adaptive_pricing: true,
    payout_currencies: ['eur', 'gbp', 'usd'],
    locale: 'en',
  },
  tap: {
    chain_id: 1,
    token_address: '0x5e7F6e008C6d9D7AD4c7EB75Bd4ce62864cc7454',
    pool_address: '0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28',
  },
  tnk: {
    network: 'mainnet',
    treasury_address: 'trac1f3w8ja3qxcnmzzmxxt8m0ystdf683sy5arnhxvz0h7a8ydd0kqwq3lcgdh',
  },
  ...overrides,
});

async function setup() {
  const admin = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract(
    { peer: { wallet: makeVerifier(outsider.wallet) } },
    {}
  );
  return { admin, outsider, storage, contract };
}

test('admin publishes one complete canonical payment directory', async () => {
  const { admin, storage, contract } = await setup();
  const first = await execute(contract, storage, 'setPayments', paymentConfig(), admin.publicKey, 1);
  assert.deepEqual(first, { ok: true, op: 'setPayments', ver: 1 });
  assert.deepEqual((await storage.get('payments/current')).value, {
    denom: 'au_usd',
    rails: ['fiat', 'tap', 'tnk'],
    fiat: {
      processor: 'stripe',
      integration_currency: 'usd',
      adaptive_pricing: true,
      payout_currencies: ['eur', 'gbp', 'usd'],
      locale: 'en',
    },
    tap: {
      chain_id: 1,
      token_address: '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454',
      pool_address: '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28',
    },
    tnk: {
      network: 'mainnet',
      treasury_address: 'trac1f3w8ja3qxcnmzzmxxt8m0ystdf683sy5arnhxvz0h7a8ydd0kqwq3lcgdh',
    },
    ver: 1,
    updated_at: '1'.padStart(64, '0'),
    set_by: admin.publicKey,
    set_by_role: 'admin',
  });

  const second = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({ ver: 2 }),
    admin.publicKey,
    2
  );
  assert.deepEqual(second, { ok: true, op: 'setPayments', ver: 2 });
  assert.equal((await storage.get('payments/current')).value.ver, 2);

  const stale = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({ ver: 2 }),
    admin.publicKey,
    3
  );
  assert.match(stale.message, /version must increase/i);
  assert.equal((await storage.get('payments/current')).value.ver, 2);
});

test('payment directory is admin-only and rejects local or incomplete discovery', async () => {
  const { admin, outsider, storage, contract } = await setup();

  const nonAdmin = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig(),
    outsider.publicKey,
    1
  );
  assert.match(nonAdmin.message, /admin required/i);
  assert.equal(await storage.get('payments/current'), null);

  const publicPaygate = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({
      fiat: {
        processor: 'stripe',
        integration_currency: 'usd',
        adaptive_pricing: true,
        payout_currencies: ['eur', 'gbp', 'usd'],
        locale: 'en',
        paygate_url: 'https://paygate.invalid',
      },
    }),
    admin.publicKey,
    2
  );
  assert.match(publicPaygate.message, /does not accept fields: paygate_url/i);

  const partial = await execute(
    contract,
    storage,
    'setPayments',
    { op: 'set_payments', ver: 1, fiat: paymentConfig().fiat, tap: paymentConfig().tap },
    admin.publicKey,
    3
  );
  assert.match(partial.message, /invalid schema/i);

  const wrongNetwork = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({
      tnk: {
        network: 'mainnet',
        treasury_address: 'testtrac1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq',
      },
    }),
    admin.publicKey,
    4
  );
  assert.match(wrongNetwork.message, /mainnet treasury must use a trac1 address/i);

  const badTap = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({ tap: { ...paymentConfig().tap, chain_id: 0 } }),
    admin.publicKey,
    5
  );
  assert.match(badTap.message, /invalid TAP chain id/i);
  assert.equal(await storage.get('payments/current'), null);
});

test('fiat config is exact, Adaptive-Pricing based, and payout-currency generic', async () => {
  const { admin, storage, contract } = await setup();
  const legacy = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({
      fiat: { processor: 'stripe', currencies: ['usd'], locale: 'en' },
    }),
    admin.publicKey,
    1
  );
  assert.match(legacy.message, /does not accept fields: currencies/i);

  for (const fiat of [
    { ...paymentConfig().fiat, integration_currency: 'eur' },
    { ...paymentConfig().fiat, adaptive_pricing: false },
    { ...paymentConfig().fiat, payout_currencies: ['eur', 'usd'] },
    { ...paymentConfig().fiat, payout_currencies: ['usd', 'gbp', 'eur'] },
    { ...paymentConfig().fiat, payout_currencies: ['eur', 'gbp', 'USD'] },
  ]) {
    const result = await execute(
      contract,
      storage,
      'setPayments',
      paymentConfig({ fiat }),
      admin.publicKey,
      2
    );
    assert.ok(result instanceof Error, JSON.stringify(result));
  }

  const generic = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({
      fiat: {
        ...paymentConfig().fiat,
        payout_currencies: ['cad', 'eur', 'gbp', 'usd'],
      },
    }),
    admin.publicKey,
    3
  );
  assert.equal(generic.ok, true, generic.message);
  assert.deepEqual(
    (await storage.get('payments/current')).value.fiat.payout_currencies,
    ['cad', 'eur', 'gbp', 'usd']
  );
});

test('rotating the canonical TAP pool isolates every pool-backed monetary record', async () => {
  const { admin, storage, contract } = await setup();
  const oldPool = paymentConfig().tap.pool_address.toLowerCase();
  const newPool = '0x1111111111111111111111111111111111111111';
  const user = '2'.repeat(64);
  const provider = '3'.repeat(64);

  const first = await execute(contract, storage, 'setPayments', paymentConfig(), admin.publicKey, 1);
  assert.equal(first.ok, true, first.message);
  await storage.put(`bal/${user}/tap`, {
    user,
    rail: 'tap',
    denom: 'au_usd',
    au: '48000000000000000000',
    updated_epoch: 7,
    updated_at: 'old-balance',
    chain_id: 1,
    pool_address: oldPool,
  });
  await storage.put(`earn/tap/${provider}`, {
    provider,
    rail: 'tap',
    denom: 'au_usd',
    total_au: '750',
    held_au: '250',
    paid_cum_au: '500',
    updated_epoch: 7,
    updated_at: 'old-earning',
    chain_id: 1,
    pool_address: oldPool,
  });
  await storage.put('fee/tap/cum', {
    rail: 'tap',
    denom: 'au_usd',
    cum_au: '150',
    swept_cum_au: '100',
    updated_epoch: 7,
    updated_at: 'old-fee',
    last_apply_hash: '4'.repeat(64),
    last_fee_bps: 1_500,
    chain_id: 1,
    pool_address: oldPool,
  });
  await storage.put('burn/tap/cum', {
    rail: 'tap',
    denom: 'au_usd',
    cum_au: '100',
    updated_epoch: 7,
    updated_at: 'old-burn',
    last_apply_hash: '4'.repeat(64),
    burn_bps: 1_000,
    chain_id: 1,
    pool_address: oldPool,
  });

  contract.storage = storage;
  assert.equal((await contract.balanceRecord(user, 'tap')).au, '48000000000000000000');
  assert.equal((await contract.earningRecord(provider, 'tap')).total_au, '750');
  assert.equal((await contract.feeCumRecord('tap')).cum_au, '150');
  assert.equal((await contract.burnCumRecord('tap')).cum_au, '100');
  contract.storage = null;

  const rotated = await execute(
    contract,
    storage,
    'setPayments',
    paymentConfig({
      ver: 2,
      tap: { ...paymentConfig().tap, pool_address: newPool },
    }),
    admin.publicKey,
    2
  );
  assert.equal(rotated.ok, true, rotated.message);

  contract.storage = storage;
  assert.deepEqual(await contract.balanceRecord(user, 'tap'), {
    user,
    rail: 'tap',
    denom: 'au_usd',
    au: '0',
    updated_epoch: 0,
    updated_at: null,
    chain_id: 1,
    pool_address: newPool,
  });
  assert.equal((await contract.earningRecord(provider, 'tap')).total_au, '0');
  assert.equal((await contract.feeCumRecord('tap')).cum_au, '0');
  assert.equal((await contract.burnCumRecord('tap')).cum_au, '0');
  contract.storage = null;
  assert.equal((await storage.get(`bal/${user}/tap`)).value.au, '48000000000000000000');
  assert.equal((await storage.get(`earn/tap/${provider}`)).value.total_au, '750');
});
