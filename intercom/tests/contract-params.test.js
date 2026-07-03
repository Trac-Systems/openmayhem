import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
} from './helpers/contract.js';

const DAY_SECONDS = 24 * 60 * 60;
const rulesHash = 'd'.repeat(64);

const makeSetParams = (overrides = {}) => ({
  op: 'set_params',
  submitted_at: 0,
  effective_at: DAY_SECONDS,
  values: {
    fee_bps: 1_200,
    price_max_bps: 20_000,
  },
  ...overrides,
});

const readParams = (at, keys = ['fee_bps', 'price_max_bps']) => ({
  op: 'read_params',
  at,
  keys,
});

test('MayhemContract setParams is admin-only and inert until the activation delay elapses', async () => {
  const admin = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({}, {});

  const nonAdminRules = await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: rulesHash },
    outsider.publicKey,
    1
  );
  assert.match(nonAdminRules.message, /admin required/i);

  const nonAdminParams = await execute(
    contract,
    storage,
    'setParams',
    makeSetParams(),
    outsider.publicKey,
    2
  );
  assert.match(nonAdminParams.message, /admin required/i);

  const tooSoon = await execute(
    contract,
    storage,
    'setParams',
    makeSetParams({ effective_at: DAY_SECONDS - 1 }),
    admin.publicKey,
    3
  );
  assert.match(tooSoon.message, /24h activation delay/i);

  const defaults = await execute(
    contract,
    storage,
    'readParams',
    readParams(0),
    outsider.publicKey,
    4
  );
  assert.deepEqual(defaults, {
    ok: true,
    op: 'readParams',
    at: 0,
    params: {
      fee_bps: 1_500,
      price_max_bps: 40_000,
    },
  });

  const scheduled = await execute(
    contract,
    storage,
    'setParams',
    makeSetParams(),
    admin.publicKey,
    5
  );
  assert.deepEqual(scheduled, {
    ok: true,
    op: 'setParams',
    ver: 1,
    effective_at: DAY_SECONDS,
    keys: ['fee_bps', 'price_max_bps'],
  });

  const beforeActivation = await execute(
    contract,
    storage,
    'readParams',
    readParams(DAY_SECONDS - 1),
    outsider.publicKey,
    6
  );
  assert.deepEqual(beforeActivation.params, {
    fee_bps: 1_500,
    price_max_bps: 40_000,
  });

  const afterActivation = await execute(
    contract,
    storage,
    'readParams',
    readParams(DAY_SECONDS),
    outsider.publicKey,
    7
  );
  assert.deepEqual(afterActivation.params, {
    fee_bps: 1_200,
    price_max_bps: 20_000,
  });

  assert.deepEqual(await storage.get('params/update/1'), {
    value: {
      ver: 1,
      values: {
        fee_bps: 1_200,
        price_max_bps: 20_000,
      },
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      set_by: admin.publicKey,
      set_by_role: 'admin',
      tx: makeTxKey(5),
    },
  });

  assert.deepEqual(await storage.get('params/current'), {
    value: {
      ver: 1,
      keys: ['fee_bps', 'price_max_bps'],
      set_by: admin.publicKey,
      set_by_role: 'admin',
      updated_at: makeTxKey(5),
      effective_at: DAY_SECONDS,
    },
  });

  assert.deepEqual(await storage.get('params/fee_bps'), {
    value: {
      key: 'fee_bps',
      current: {
        value: 1_500,
        ver: 0,
        submitted_at: 0,
        effective_at: 0,
        set_at: null,
      },
      pending: {
        value: 1_200,
        ver: 1,
        submitted_at: 0,
        effective_at: DAY_SECONDS,
        set_by: admin.publicKey,
        set_by_role: 'admin',
        set_at: makeTxKey(5),
      },
    },
  });
});
