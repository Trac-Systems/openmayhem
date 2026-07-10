import assert from 'node:assert/strict';
import test from 'node:test';

import MayhemContract from '../contract/contract.js';
import { MemoryStorage, execute, makeIdentity, makeVerifier } from './helpers/contract.js';

const paymentConfig = (overrides = {}) => ({
  op: 'set_payments',
  ver: 1,
  fiat: {
    processor: 'stripe',
    currencies: ['eur', 'usd'],
    locale: 'en',
  },
  tap: {
    chain_id: 1,
    token_address: '0x5e7F6e008C6d9D7AD4c7EB75Bd4ce62864cc7454',
    pool_address: '0x9B254d37C28Fb5893F46513a61925eDC2F300615',
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
      currencies: ['usd', 'eur'],
      locale: 'en',
    },
    tap: {
      chain_id: 1,
      token_address: '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454',
      pool_address: '0x9b254d37c28fb5893f46513a61925edc2f300615',
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
        currencies: ['usd', 'eur'],
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
