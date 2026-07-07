import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { contractParamDefinitions } from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
} from './helpers/contract.js';

const DAY_SECONDS = 24 * 60 * 60;
const rulesHash = 'd'.repeat(64);
const EPOCH_OPERATING_PARAM_VALUES = {
  epoch_seconds: 7_200,
  challenge_epochs: 3,
  holdback_epochs: 12,
  max_apply_batch: 2_500,
  max_tnk_settlement_outputs: 100,
  param_activation_delay_seconds: 3_600,
  rules_grace_seconds: 300,
  rate_staleness_seconds: 120,
  uptime_tick_seconds: 1_800,
  fraud_slash_bps: 5_000,
  dispute_lost_slash_bps: 1_000,
  price_rate_limit_seconds: 900,
  market_target_utilization_bps: 7_500,
  market_ema_alpha_bps: 4_000,
  market_gain_bps: 6_000,
  market_max_step_bps: 1_500,
  market_cold_start_min_providers: 5,
  market_provider_epoch_target_mu: 2_000_000,
  market_max_utilization_bps: 60_000,
  market_below_target_discount_bps: 1_000,
  market_above_target_slope_bps: 20_000,
};

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
  assert.match(tooSoon.message, /param_activation_delay_seconds/i);

  const defaults = await execute(
    contract,
    storage,
    'readParams',
    readParams(0, [
      'fee_bps',
      'price_max_bps',
      'epoch_seconds',
      'challenge_epochs',
      'holdback_epochs',
      'max_apply_batch',
      'max_tnk_settlement_outputs',
      'dispute_deposit_mu',
      'rate_staleness_seconds',
      'uptime_tick_seconds',
      'fraud_slash_bps',
      'dispute_lost_slash_bps',
      'price_rate_limit_seconds',
      'param_activation_delay_seconds',
      'market_target_utilization_bps',
      'market_provider_epoch_target_mu',
    ]),
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
      epoch_seconds: 3_600,
      challenge_epochs: 6,
      holdback_epochs: 24,
      max_apply_batch: 2_000,
      max_tnk_settlement_outputs: 5_000,
      dispute_deposit_mu: 1_000_000,
      rate_staleness_seconds: 2_700,
      uptime_tick_seconds: 21_600,
      fraud_slash_bps: 10_000,
      dispute_lost_slash_bps: 2_000,
      price_rate_limit_seconds: 21_600,
      param_activation_delay_seconds: 86_400,
      market_target_utilization_bps: 8_500,
      market_provider_epoch_target_mu: 1_000_000,
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

  const aboveFeeCap = await execute(
    contract,
    storage,
    'setParams',
    makeSetParams({
      submitted_at: DAY_SECONDS,
      effective_at: 2 * DAY_SECONDS,
      values: { fee_bps: 5_001 },
    }),
    admin.publicKey,
    8
  );
  assert.match(aboveFeeCap.message, /fee_bps.*out of range/i);

  const tuneEpochAndMarket = await execute(
    contract,
    storage,
    'setParams',
    makeSetParams({
      submitted_at: DAY_SECONDS,
      effective_at: 2 * DAY_SECONDS,
      values: {
        epoch_seconds: 7_200,
        challenge_epochs: 3,
        holdback_epochs: 12,
        max_apply_batch: 2_500,
        max_tnk_settlement_outputs: 100,
        dispute_deposit_mu: 2_000_000,
        rate_staleness_seconds: 120,
        uptime_tick_seconds: 1_800,
        price_rate_limit_seconds: 900,
        market_target_utilization_bps: 7_500,
        market_provider_epoch_target_mu: 2_000_000,
        param_activation_delay_seconds: 3_600,
      },
    }),
    admin.publicKey,
    9
  );
  assert.equal(tuneEpochAndMarket.ok, true, tuneEpochAndMarket.message);

  const tuned = await execute(
    contract,
    storage,
    'readParams',
    readParams(2 * DAY_SECONDS, [
      'epoch_seconds',
      'challenge_epochs',
      'holdback_epochs',
      'max_apply_batch',
      'max_tnk_settlement_outputs',
      'dispute_deposit_mu',
      'rate_staleness_seconds',
      'uptime_tick_seconds',
      'price_rate_limit_seconds',
      'market_target_utilization_bps',
      'market_provider_epoch_target_mu',
      'param_activation_delay_seconds',
    ]),
    outsider.publicKey,
    10
  );
  assert.deepEqual(tuned.params, {
    epoch_seconds: 7_200,
    challenge_epochs: 3,
    holdback_epochs: 12,
    max_apply_batch: 2_500,
    max_tnk_settlement_outputs: 100,
    dispute_deposit_mu: 2_000_000,
    rate_staleness_seconds: 120,
    uptime_tick_seconds: 1_800,
    price_rate_limit_seconds: 900,
    market_target_utilization_bps: 7_500,
    market_provider_epoch_target_mu: 2_000_000,
    param_activation_delay_seconds: 3_600,
  });
});

test('MayhemContract epoch and market epoch controls are admin-governed params', async () => {
  const admin = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({}, {});
  const definitions = contractParamDefinitions();

  for (const [key, value] of Object.entries(EPOCH_OPERATING_PARAM_VALUES)) {
    const definition = definitions[key];
    assert.ok(definition, `${key} must be registered in contract PARAM_DEFINITIONS`);
    assert.equal(Number.isInteger(definition.default), true, `${key} default must be integer`);
    assert.equal(Number.isInteger(definition.min), true, `${key} min must be integer`);
    assert.equal(Number.isInteger(definition.max), true, `${key} max must be integer`);
    assert.ok(value >= definition.min && value <= definition.max, `${key} test value must fit bounds`);
  }

  const providerAttempt = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: EPOCH_OPERATING_PARAM_VALUES,
    },
    outsider.publicKey,
    1
  );
  assert.match(providerAttempt.message, /admin required/i);

  const scheduled = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: EPOCH_OPERATING_PARAM_VALUES,
    },
    admin.publicKey,
    2
  );
  assert.deepEqual(scheduled, {
    ok: true,
    op: 'setParams',
    ver: 1,
    effective_at: DAY_SECONDS,
    keys: Object.keys(EPOCH_OPERATING_PARAM_VALUES).sort(),
  });

  const inactive = await execute(
    contract,
    storage,
    'readParams',
    readParams(DAY_SECONDS - 1, Object.keys(EPOCH_OPERATING_PARAM_VALUES)),
    outsider.publicKey,
    3
  );
  assert.notDeepEqual(inactive.params, EPOCH_OPERATING_PARAM_VALUES);

  const active = await execute(
    contract,
    storage,
    'readParams',
    readParams(DAY_SECONDS, Object.keys(EPOCH_OPERATING_PARAM_VALUES)),
    outsider.publicKey,
    4
  );
  assert.deepEqual(active.params, EPOCH_OPERATING_PARAM_VALUES);
});
