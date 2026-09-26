import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import { MemoryStorage, execute, makeIdentity, seedCurrentAdminPrice } from './helpers/contract.js';

const ENCLAVE = 'ac'.repeat(32);
const MODEL = 'test/activity-market';
const calibration = {
  schema_version: 1, source_hash: 'ca'.repeat(32), dimensions: [
    { unit: 'input_token', units: '1000', work_us: '1000000' },
    { unit: 'output_token', units: '100', work_us: '1000000' },
  ],
};

async function market({
  calibrated = false,
  modelClass = 'text-generation',
  units = ['input_token', 'output_token'],
  fixedTerms = false,
} = {}) {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({}, {});
  contract.storage = storage;
  contract.address = admin.publicKey;
  contract.tx = 'aa'.repeat(32);
  const ctxBracket = modelClass === 'text-generation' ? 'le8k' : null;
  const priceKey = `price/${ENCLAVE}${ctxBracket ? '/'+ctxBracket : ''}`;
  const rates = units.map((unit) => ({ unit, per_unit_au: '1000000', granularity: 1 }));
  await storage.put(`enclave/${ENCLAVE}`, {
    enclave_id: ENCLAVE,
    model_id: MODEL,
    model_class: modelClass,
    status: 'active',
    caps: { ctx: 8192, modality_set: ['text'] },
  });
  await storage.put(`modelref/${MODEL}`, {
    model_id: MODEL,
    model_class: modelClass,
    ver: 1,
    rate_map: rates,
    ...(calibrated ? { activity_calibration: calibration } : {}),
  });
  await seedCurrentAdminPrice(storage, {
    enclaveId: ENCLAVE,
    modelId: MODEL,
    admin: admin.publicKey,
    rateMap: rates,
    perReqAu: fixedTerms ? 1000000 : 0,
    minSessionAu: fixedTerms ? 2000000 : 0,
    ctxBracket,
    ctxBracketTableVer: ctxBracket ? 1 : null,
  });

  async function step(epoch, utilizationBps, {
    gross = '100',
    providers = 1,
    capacitySlots = providers,
    seconds = 3600,
    computeMs = null,
    legacyReceiptCount = 0,
    counts = { [units[0]]: '1' },
    persist = true,
  } = {}) {
    const calculatedBusyMs = BigInt(seconds) * 1000n * BigInt(capacitySlots) *
      BigInt(utilizationBps) / 10000n;
    const busyMs = computeMs === null
      ? (calculatedBusyMs > 0n ? calculatedBusyMs : 1n).toString()
      : String(computeMs);
    const row = {
      enclave_id: ENCLAVE,
      ...(ctxBracket ? { ctx_bracket: ctxBracket, ctx_bracket_table_ver: 1 } : {}),
      demand_au: gross,
      session_count: 2,
      provider_count: providers,
      compute_ms: busyMs,
      capacity_slot_count: capacitySlots,
      legacy_receipt_count: legacyReceiptCount,
    };
    const usage = contract.aggregateMarketUsageEntries([row]);
    assert.ok(!(usage instanceof Error), usage.message);
    const key = contract.priceMarketKey(ENCLAVE, ctxBracket);
    const result = await contract.computeMarketPriceUpdates(usage, {
      epoch,
      at: epoch * seconds,
      epochSeconds: seconds,
      canonicalActivity: new Map([[key, { ...row, settled_usage: counts }]]),
      includeDormant: true,
    });
    assert.ok(!(result instanceof Error), result.message);
    assert.equal(result.length, 1);
    if (persist) {
      await storage.put(result[0].schedule_key, result[0].schedule);
      await storage.put(result[0].record_key, result[0].record);
      await contract.writeActivityMarketIndex(result);
    }
    return result[0];
  }

  return { contract, storage, admin, step, priceKey };
}

const amount = (update) => BigInt(update.rate_map[0].per_unit_au);

test('20 and 80 percent boundaries apply exact fixed price steps', async () => {
  for (const [utilization, expected, multiplier] of [
    [0, 900000n, 9000],
    [2000, 900000n, 9000],
    [2001, 1000000n, 10000],
    [7999, 1000000n, 10000],
    [8000, 1100000n, 11000],
    [10000, 1100000n, 11000],
  ]) {
    const ctx = await market();
    const update = await ctx.step(1, utilization);
    assert.equal(amount(update), expected, String(utilization));
    assert.equal(update.utilization_bps, utilization);
    assert.equal(update.multiplier_bps, multiplier);
    assert.equal(update.record.market.activity_basis, 'signed_slot_time_v1');
    assert.equal(update.record.price_source, 'market_utilization');
  }
});

test('sustained saturation rises every epoch and sustained idleness falls every epoch', async () => {
  const rising = await market();
  let prior = 1000000n;
  for (let epoch = 1; epoch <= 20; epoch++) {
    const update = await rising.step(epoch, 8000);
    assert.ok(amount(update) >= prior);
    assert.ok(amount(update) - prior <= prior / 10n + 1n);
    prior = amount(update);
  }
  assert.equal(prior, 4000000n);

  const falling = await market();
  prior = 1000000n;
  for (let epoch = 1; epoch <= 30; epoch++) {
    const update = await falling.step(epoch, 2000);
    assert.ok(amount(update) <= prior);
    assert.ok(prior - amount(update) <= prior / 10n + 1n);
    prior = amount(update);
  }
  assert.equal(prior, 250000n);
});

test('price direction depends on absolute utilization, not the previous hour', async () => {
  const high = await market();
  const highOne = await high.step(1, 8500);
  const highTwo = await high.step(2, 8500);
  assert.equal(amount(highOne), 1100000n);
  assert.equal(amount(highTwo), 1210000n);

  const low = await market();
  const lowOne = await low.step(1, 1500);
  const lowTwo = await low.step(2, 1500);
  assert.equal(amount(lowOne), 900000n);
  assert.equal(amount(lowTwo), 810000n);
});

test('an epoch containing a pre-v27 receipt settles while holding price once', async () => {
  const ctx = await market();
  const first = await ctx.step(1, 8000);
  assert.equal(amount(first), 1100000n);

  const held = await ctx.step(2, 0, {
    computeMs: 0,
    capacitySlots: 0,
    legacyReceiptCount: 1,
  });
  assert.equal(amount(held), 1100000n);
  assert.equal(held.utilization_bps, null);
  assert.equal(held.multiplier_bps, 10000);
  assert.equal(held.record.market.activity_basis, 'legacy_receipt_hold_v1');
  assert.equal(held.record.market.legacy_receipt_count, 1);

  const resumed = await ctx.step(3, 8000);
  assert.equal(amount(resumed), 1210000n);
  assert.equal(resumed.record.market.activity_basis, 'signed_slot_time_v1');
});

test('mid-band utilization holds regardless of demand AU or metered-unit changes', async () => {
  const ctx = await market();
  const first = await ctx.step(1, 5000, {
    gross: '1',
    counts: { input_token: '1', output_token: '5000' },
  });
  const second = await ctx.step(2, 5000, {
    gross: '999999999999999999',
    counts: { input_token: '999999999', output_token: '1' },
  });
  assert.equal(amount(first), 1000000n);
  assert.equal(amount(second), 1000000n);
});

test('slot capacity normalizes the same compute time into different utilization', async () => {
  const oneSlot = await market();
  const twoSlots = await market();
  const computeMs = 2_880_000;
  const busy = await oneSlot.step(1, 0, { capacitySlots: 1, computeMs });
  const spare = await twoSlots.step(1, 0, { capacitySlots: 2, computeMs });
  assert.equal(busy.utilization_bps, 8000);
  assert.equal(amount(busy), 1100000n);
  assert.equal(spare.utilization_bps, 4000);
  assert.equal(amount(spare), 1000000n);
});

test('utilization is invariant to epoch length and provider count when slot time matches', async () => {
  const short = await market();
  const long = await market();
  const a = await short.step(1, 8000, { seconds: 1800, providers: 1, capacitySlots: 2 });
  const b = await long.step(1, 8000, { seconds: 3600, providers: 4, capacitySlots: 8 });
  assert.equal(a.utilization_bps, b.utilization_bps);
  assert.deepEqual(a.rate_map, b.rate_map);
});

test('every model class uses the same signed slot-time controller without calibration', async () => {
  for (const [modelClass, units] of [
    ['text-generation', ['input_token','output_token']],
    ['embedding', ['input_token','embedding']],
    ['workflow', ['pixel_frame']],
    ['image-generation', ['image','step']],
    ['video-generation', ['frame','video_second']],
    ['tts', ['audio_second','input_character']],
    ['stt', ['audio_second']],
    ['audio-generation', ['audio_second','input_character']],
    ['music-generation', ['audio_second','input_character']],
  ]) {
    const ctx = await market({ calibrated: false, modelClass, units });
    const up = await ctx.step(1, 8000, {
      counts: Object.fromEntries(units.map((unit) => [unit, '1'])),
    });
    assert.equal(up.record.market.activity_basis, 'signed_slot_time_v1', modelClass);
    assert.equal(amount(up), 1100000n, modelClass);
  }
});

test('consecutive empty epochs decay dormant markets to the hard floor', async () => {
  const ctx = await market({ fixedTerms: true });
  await ctx.step(1, 5000);
  let previous = 1000000n;
  for (let epoch = 2; epoch <= 30; epoch++) {
    const updates = await ctx.contract.computeMarketPriceUpdates(new Map(), {
      epoch,
      at: epoch * 3600,
      epochSeconds: 3600,
      canonicalActivity: new Map(),
      includeDormant: true,
    });
    assert.ok(!(updates instanceof Error), updates.message);
    const next = updates[0];
    await ctx.storage.put(next.schedule_key, next.schedule);
    assert.equal(next.utilization_bps, 0);
    assert.equal(next.record.market.capacity_slot_count, 0);
    assert.ok(amount(next) >= 250000n);
    previous = amount(next);
  }
  assert.equal(previous, 250000n);
});

test('canonical compute evidence must match the committed market totals', async () => {
  const ctx = await market();
  const row = {
    enclave_id: ENCLAVE,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    demand_au: '100',
    session_count: 1,
    provider_count: 1,
    compute_ms: '2880000',
    capacity_slot_count: 1,
  };
  const usage = ctx.contract.aggregateMarketUsageEntries([row]);
  const key = ctx.contract.priceMarketKey(ENCLAVE, 'le8k');
  const result = await ctx.contract.computeMarketPriceUpdates(usage, {
    epoch: 1,
    at: 3600,
    epochSeconds: 3600,
    canonicalActivity: new Map([[key, {
      ...row,
      compute_ms: '2879999',
      settled_usage: { input_token: '1' },
    }]]),
  });
  assert.match(result.message, /do not match canonical receipt evidence/);
});

test('market usage rejects missing, zero, or malformed slot-time evidence', async () => {
  const ctx = await market();
  const base = {
    enclave_id: ENCLAVE,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    demand_au: '1',
    session_count: 1,
    provider_count: 1,
    compute_ms: '1',
    capacity_slot_count: 1,
  };
  for (const invalid of [
    (({ compute_ms, ...row }) => row)(base),
    { ...base, compute_ms: '0' },
    { ...base, compute_ms: 'nope' },
    { ...base, capacity_slot_count: 0 },
  ]) {
    assert.ok(ctx.contract.aggregateMarketUsageEntries([invalid]) instanceof Error);
  }
});

test('price derivation hashes bind compute time, capacity, utilization and result', async () => {
  const ctx = await market();
  const update = await ctx.step(1, 8000);
  const derivation = ctx.contract.priceDerivationFromMarketUpdate(update, {
    epoch: 1,
    at: 3600,
    epochSeconds: 3600,
    usageRoot: 'ab'.repeat(32),
  });
  const original = await ctx.contract.priceDerivationLeafHash(derivation);
  for (const change of [
    (value) => { value.usage.compute_ms = '1'; },
    (value) => { value.usage.capacity_slot_count = 9; },
    (value) => { value.controller.utilization_bps = 1; },
    (value) => { value.result_price.rate_map[0].per_unit_au = '1'; },
  ]) {
    const tampered = structuredClone(derivation);
    change(tampered);
    assert.notEqual(await ctx.contract.priceDerivationLeafHash(tampered), original);
  }
});

test('fixed request terms follow utilization and stay inside seed bounds', async () => {
  const ctx = await market({ fixedTerms: true });
  let last;
  for (let epoch = 1; epoch <= 30; epoch++) last = await ctx.step(epoch, 10000);
  assert.equal(amount(last), 4000000n);
  assert.equal(BigInt(last.record.per_req_au), 4000000n);
  assert.equal(BigInt(last.record.min_session_au), 8000000n);
  for (let epoch = 31; epoch <= 80; epoch++) last = await ctx.step(epoch, 0);
  assert.equal(amount(last), 250000n);
  assert.equal(BigInt(last.record.per_req_au), 250000n);
  assert.equal(BigInt(last.record.min_session_au), 500000n);
});

test('hard-band migration preserves history and records utilization policy', async () => {
  const ctx = await market();
  const records = {};
  for (const [key, active, pending] of [
    ['price_min_bps', 2500, 1],
    ['price_max_bps', 40000, 1000000],
  ]) {
    records[key] = {
      key,
      current: { value: active, ver: 1, effective_at: 0 },
      pending: { value: pending, ver: 2, effective_at: 86400 },
    };
    await ctx.storage.put(`params/${key}`, records[key]);
  }
  await ctx.storage.put('params/update/2', {
    values: { price_min_bps: 1, price_max_bps: 1000000 },
  });
  const outsider = await makeIdentity();
  const rejected = await execute(ctx.contract, ctx.storage, 'migrateMarketPricing', {
    op: 'migrate_market_pricing', at: 0, markets: [],
  }, outsider.publicKey, 1);
  assert.match(rejected.message, /admin/i);
  const applied = await execute(ctx.contract, ctx.storage, 'migrateMarketPricing', {
    op: 'migrate_market_pricing', at: 0, markets: [],
  }, ctx.admin.publicKey, 2);
  assert.equal(applied.repaired, 2);
  const migration = (await ctx.storage.get('market/activity/migration-v3')).value;
  assert.equal(migration.schema_version, 3);
  assert.equal(migration.low_utilization_bps, 2000);
  assert.equal(migration.high_utilization_bps, 8000);
  assert.equal(migration.price_step_bps, 1000);
  assert.deepEqual((await ctx.storage.get('params/update/2')).value.values, {
    price_min_bps: 1,
    price_max_bps: 1000000,
  });
  const repeated = await execute(ctx.contract, ctx.storage, 'migrateMarketPricing', {
    op: 'migrate_market_pricing', at: 0, markets: [],
  }, ctx.admin.publicKey, 3);
  assert.equal(repeated.idempotent, true);
});

test('migration rejects partial epoch upgrades without modifying parameters', async () => {
  const ctx = await market();
  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: 1,
    pending_epoch: 2,
    pending_next_page: 1,
  });
  const result = await execute(ctx.contract, ctx.storage, 'migrateMarketPricing', {
    op: 'migrate_market_pricing', at: 0, markets: [],
  }, ctx.admin.publicKey, 1);
  assert.match(result.message, /completed epoch boundary/);
  assert.equal(await ctx.storage.get('market/activity/migration-v3'), null);
});

test('migration seeds active base and context markets before empty utilization updates', async () => {
  const ctx = await market();
  const base = 'ad'.repeat(32);
  const baseModel = 'test/media';
  const rates = [{ unit: 'pixel_frame', per_unit_au: '1000', granularity: 1 }];
  await ctx.storage.put(`enclave/${base}`, {
    enclave_id: base,
    model_id: baseModel,
    model_class: 'workflow',
    status: 'active',
    caps: {},
  });
  await ctx.storage.put(`modelref/${baseModel}`, {
    model_id: baseModel,
    model_class: 'workflow',
    rate_map: rates,
  });
  await seedCurrentAdminPrice(ctx.storage, {
    enclaveId: base,
    modelId: baseModel,
    admin: ctx.admin.publicKey,
    rateMap: rates,
    ctxBracket: null,
  });
  const markets = [
    { enclave_id: base },
    { enclave_id: ENCLAVE, ctx_bracket: 'le8k', ctx_bracket_table_ver: 1 },
  ];
  const migration = await execute(ctx.contract, ctx.storage, 'migrateMarketPricing', {
    op: 'migrate_market_pricing', at: 0, markets,
  }, ctx.admin.publicKey, 1);
  assert.equal(migration.market_count, 2);
  ctx.contract.storage = ctx.storage;
  const empty = await ctx.contract.computeMarketPriceUpdates(new Map(), {
    epoch: 1,
    at: 3600,
    epochSeconds: 3600,
    includeDormant: true,
    canonicalActivity: new Map(),
  });
  assert.equal(empty.length, 2);
  assert.ok(empty.every((update) => update.utilization_bps === 0));
});

test('activity calibration remains valid metadata but no longer controls direction', async () => {
  const ctx = await market({ calibrated: true });
  const rates = [{ unit: 'input_token' }, { unit: 'output_token' }];
  assert.equal(ctx.contract.validateActivityCalibration(calibration, 'text-generation', rates), null);
  const partial = structuredClone(calibration);
  partial.dimensions.pop();
  assert.match(
    ctx.contract.validateActivityCalibration(partial, 'text-generation', rates).message,
    /every model reference/
  );
  const update = await ctx.step(1, 8000, { counts: { input_token: '1' } });
  assert.equal(update.record.market.activity_basis, 'signed_slot_time_v1');
  assert.equal(amount(update), 1100000n);
});

test('activity index overflow fails before a price write', async () => {
  const ctx = await market();
  await ctx.storage.put(
    'market/activity/index',
    Array.from({ length: 5000 }, (_, index) => ({ enclave_id: `market-${index}` }))
  );
  const before = ctx.storage.snapshotBytes();
  const row = {
    enclave_id: ENCLAVE,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    demand_au: '10',
    session_count: 1,
    provider_count: 1,
    compute_ms: '1',
    capacity_slot_count: 1,
  };
  const result = await ctx.contract.computeMarketPriceUpdates(
    ctx.contract.aggregateMarketUsageEntries([row]),
    { epoch: 1, at: 3600, epochSeconds: 3600 }
  );
  assert.match(result.message, /capacity exceeded/);
  assert.equal(ctx.storage.snapshotBytes(), before);
});
