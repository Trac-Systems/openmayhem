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
      await storage.put(result[0].demand_key, result[0].demand_state);
      await contract.writeActivityMarketIndex(result);
    }
    return result[0];
  }

  return { contract, storage, admin, step, priceKey };
}

const amount = (update) => BigInt(update.rate_map[0].per_unit_au);

async function warm(ctx, counts) {
  for (let epoch = 1; epoch <= 72; epoch++) {
    const update = await ctx.step(epoch, 8000, {counts});
    assert.equal(update.record.market.multiplier_bps, null);
  }
}

test('every model class uses paid demand, independently of occupancy, spend and session count', async () => {
  for (const [modelClass, units] of [
    ['text-generation', ['input_token', 'output_token']],
    ['decision', ['input_token', 'output_token']],
    ['embedding', ['input_token']],
    ['workflow', ['pixel_frame']],
    ['image-generation', ['image', 'step']],
    ['video-generation', ['frame', 'video_second']],
    ['tts', ['audio_second', 'input_character']],
    ['stt', ['audio_second']],
    ['audio-generation', ['audio_second', 'input_character']],
    ['music-generation', ['audio_second', 'input_character']],
  ]) {
    const ctx = await market({modelClass, units});
    const counts = Object.fromEntries(units.map((unit) => [unit, '100']));
    await warm(ctx, counts);
    const first = await ctx.step(73, 0, {counts, gross: '1', capacitySlots: 100});
    assert.equal(amount(first), 4000000n, modelClass);
    assert.equal(first.record.price_source, 'market_reference_demand');
    assert.equal(first.record.market.activity_basis, 'frozen_reference_paid_units_v1');
    let idle;
    for (let epoch = 74; epoch < 80; epoch++) {
      idle = await ctx.step(epoch, 10000, {counts: Object.fromEntries(units.map((u) => [u, '0']))});
      // A paid session with missing units holds. This step fixture declares 2.
      assert.equal(amount(idle), 4000000n);
    }
    const resumed = await ctx.step(80, 0, {counts: Object.fromEntries(units.map((u) => [u, '1']))});
    assert.ok(amount(resumed) < 4000000n);
  }
});

test('new markets hold their existing quote through reference formation; targets ignore prior prices', async () => {
  const ctx = await market({fixedTerms: true});
  await warm(ctx, {input_token: '100', output_token: '100'});
  const update = await ctx.step(73, 0, {counts: {input_token: '100', output_token: '100'}});
  assert.equal(update.per_req_au, undefined);
  assert.equal(update.record.per_req_au, '4000000');
  assert.equal(update.record.min_session_au, '8000000');
  assert.equal(update.record.market.demand_reference.paid_observations, 72);
  assert.equal(update.record.market.demand_reference_version, 1);
  const ref = structuredClone(update.record.market.demand_reference);
  const held = await ctx.step(74, 0, {counts: {unrecognized: '1'}});
  assert.equal(held.record.market.demand_status, 'unrecognized_paid_axis');
  assert.deepEqual(held.record.market.demand_reference, ref);
  assert.equal(amount(held), 4000000n);
});

test('dormant markets append known zero work and leave the ceiling after one empty epoch', async () => {
  const ctx = await market();
  await warm(ctx, {input_token: '100', output_token: '100'});
  await ctx.step(73, 0, {counts: {input_token: '100', output_token: '100'}});
  const result = await ctx.contract.computeMarketPriceUpdates(new Map(), {
    epoch: 74, at: 74 * 3600, epochSeconds: 3600, includeDormant: true, canonicalActivity: new Map(),
  });
  assert.ok(!(result instanceof Error), result.message);
  assert.equal(result.length, 1);
  assert.equal(amount(result[0]), 3375000n);
  assert.equal(result[0].demand_state.history.length, 72);
  assert.equal(result[0].demand_state.history.at(-1), '0');
});

test('demand update has bounded exact reads and never enumerates receipts or price history', async () => {
  const ctx = await market();
  await warm(ctx, {input_token: '100', output_token: '100'});
  const gets = [];
  const get = ctx.storage.get.bind(ctx.storage);
  ctx.storage.get = async (key) => { gets.push(key); return get(key); };
  const update = await ctx.step(73, 5000, {counts: {input_token: '1', output_token: '1'}, persist: false});
  assert.ok(!(update instanceof Error));
  assert.ok(gets.length < 40, String(gets.length));
  assert.equal(gets.filter((key) => key.startsWith('market/demand/')).length, 1);
  assert.ok(!gets.some((key) => /^(receipt\/|market\/price\/|ev\/price\/)/.test(key)));
});

test('one-time bootstrap re-reads canonical units, is admin-only, and cannot replace an active reference', async () => {
  const ctx = await market();
  await ctx.storage.put('epoch/apply/state', {updated_epoch: 100, pending_epoch: null});
  for (let epoch = 1; epoch <= 73; epoch++) await ctx.storage.put(`market/price/${epoch}/${ENCLAVE}/le8k`, {
    type: 'price_derivation', epoch, epoch_seconds: 3600, enclave_id: ENCLAVE,
    model_id: MODEL, ctx_bracket: 'le8k', derivation_hash: 'aa'.repeat(32),
    usage: {session_count: 2, settled_usage: {input_token: '100', output_token: '100'}},
  });
  const command = {op: 'bootstrap_market_demand', at: 360000, enclave_id: ENCLAVE,
    ctx_bracket: 'le8k', ctx_bracket_table_ver: 1, through_epoch: 100, expected_price_ver: 1,
    reference_epochs: Array.from({length: 72}, (_, i) => i + 1), history_epochs: [73], source_hash: 'ab'.repeat(32)};
  const outsider = await makeIdentity();
  const denied = await execute(ctx.contract, ctx.storage, 'bootstrapMarketDemand', command, outsider.publicKey, 1);
  assert.ok(denied instanceof Error);
  const applied = await execute(ctx.contract, ctx.storage, 'bootstrapMarketDemand', command, ctx.admin.publicKey, 2);
  assert.equal(applied.ok, true, applied.message);
  const repeat = await execute(ctx.contract, ctx.storage, 'bootstrapMarketDemand', command, ctx.admin.publicKey, 3);
  assert.equal(repeat.idempotent, true, repeat.message);
  const changed = await execute(ctx.contract, ctx.storage, 'bootstrapMarketDemand', {...command, source_hash: 'cd'.repeat(32)}, ctx.admin.publicKey, 4);
  assert.match(changed.message, /rebaselining/);
  assert.equal((await ctx.storage.get(ctx.priceKey)).value.current.ver, 1);
  ctx.contract.storage = ctx.storage;
  const update = await ctx.step(101, 0, {counts: {input_token: '100', output_token: '100'}});
  assert.equal(amount(update), 4000000n);
});

test('bootstrap rejects stale boundaries, changed prices, malformed evidence and context without writes', async () => {
  for (const scenario of ['pending', 'boundary', 'price', 'context', 'identity', 'missing', 'unknown', 'duplicate', 'zero_start', 'active']) {
    const ctx = await market();
    await ctx.storage.put('epoch/apply/state', {updated_epoch: 100, pending_epoch: scenario === 'pending' ? 101 : null});
    const evidenceKey = `market/price/1/${ENCLAVE}/le8k`;
    const row = {type: 'price_derivation', epoch: 1, epoch_seconds: 3600, enclave_id: ENCLAVE,
      model_id: scenario === 'identity' ? 'another/model' : MODEL, ctx_bracket: 'le8k',
      derivation_hash: 'aa'.repeat(32), usage: {session_count: scenario === 'zero_start' ? 0 : 1,
        settled_usage: ['unknown', 'zero_start'].includes(scenario) ? {} : {input_token: '100'}}};
    if (scenario !== 'missing') await ctx.storage.put(evidenceKey, row);
    const command = {op: 'bootstrap_market_demand', at: 360000, enclave_id: ENCLAVE,
      ctx_bracket: 'le8k', ctx_bracket_table_ver: scenario === 'context' ? 99 : 1,
      through_epoch: scenario === 'boundary' ? 99 : 100, expected_price_ver: scenario === 'price' ? 2 : 1,
      reference_epochs: scenario === 'duplicate' ? [1, 1] : [1], history_epochs: [], source_hash: 'ab'.repeat(32)};
    const demandKey = `market/demand/${ctx.contract.priceMarketKey(ENCLAVE, 'le8k')}`;
    if (scenario === 'active') await ctx.storage.put(demandKey, {schema_version: 1});
    const before = ctx.storage.snapshotBytes();
    const result = await execute(ctx.contract, ctx.storage, 'bootstrapMarketDemand', command, ctx.admin.publicKey, 9);
    assert.ok(result instanceof Error, scenario);
    assert.equal(ctx.storage.snapshotBytes(), before, scenario);
  }
});
