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
async function market({ calibrated = true, modelClass = 'text-generation', units = ['input_token', 'output_token'], fixedTerms = false } = {}) {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({}, {});
  contract.storage = storage; contract.address = admin.publicKey; contract.tx = 'aa'.repeat(32);
  const ctxBracket = modelClass === 'text-generation' ? 'le8k' : null;
  const priceKey = `price/${ENCLAVE}${ctxBracket ? '/'+ctxBracket : ''}`;
  const rates = units.map((unit) => ({ unit, per_unit_au: '1000000', granularity: 1 }));
  await storage.put(`enclave/${ENCLAVE}`, {
    enclave_id: ENCLAVE, model_id: MODEL, model_class: modelClass, status: 'active',
    caps: { ctx: 8192, modality_set: ['text'] },
  });
  await storage.put(`modelref/${MODEL}`, { model_id: MODEL, model_class: modelClass,
    ver: 1, rate_map: rates, ...(calibrated ? { activity_calibration: calibration } : {}) });
  await seedCurrentAdminPrice(storage, { enclaveId: ENCLAVE, modelId: MODEL,
    admin: admin.publicKey, rateMap: rates, perReqAu: fixedTerms ? 1000000 : 0, minSessionAu: fixedTerms ? 2000000 : 0, ctxBracket, ctxBracketTableVer: ctxBracket ? 1 : null });
  async function step(epoch, counts, { gross = '100', providers = 2, seconds = 3600, persist = true } = {}) {
    const row = { enclave_id: ENCLAVE,
      ...(ctxBracket ? { ctx_bracket: ctxBracket, ctx_bracket_table_ver: 1 } : {}),
      demand_au: gross, session_count: 2, provider_count: providers };
    const usage = contract.aggregateMarketUsageEntries([row]);
    assert.ok(!(usage instanceof Error), usage.message);
    const key = contract.priceMarketKey(ENCLAVE, ctxBracket);
    const result = await contract.computeMarketPriceUpdates(usage, {
      epoch, at: epoch * seconds, epochSeconds: seconds,
      canonicalActivity: new Map([[key, { ...row, settled_usage: counts }]]), includeDormant: true,
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

test('calibrated prefill/decode work drives direction independently of gross spend and provider count', async () => {
  const a = await market(); const b = await market();
  await a.step(1, { input_token: '1000', output_token: '100' });
  await b.step(1, { input_token: '1000', output_token: '100' }, { gross: '999999999', providers: 200 });
  const up = await a.step(2, { input_token: '2000', output_token: '200' }, { gross: '1' });
  const expensive = await b.step(2, { input_token: '2000', output_token: '200' }, { gross: '999999999999', providers: 500 });
  assert.equal(up.record.market.calibrated_work_ps, '4000000000000');
  assert.equal(up.momentum_bps, 20000); assert.equal(amount(up), 1100000n);
  assert.deepEqual(up.rate_map, expensive.rate_map);
  const down = await a.step(3, { input_token: '500', output_token: '50' }, { gross: '999999999' });
  assert.ok(down.momentum_bps < 10000); assert.equal(amount(down), 990000n);
  assert.ok(!Object.hasOwn(down.record.market.constants, 'provider_epoch_target_au'));
});

test('equal calibrated work with a different prompt/decode mix keeps the same price', async () => {
  const ctx = await market();
  await ctx.step(1, { input_token: '1000', output_token: '100' });
  const stable = await ctx.step(2, { input_token: '2000' });
  assert.equal(stable.momentum_bps, 10000); assert.equal(amount(stable), 1000000n);
});

test('missing calibration uses dimension-relative momentum for every model class', async () => {
  for (const [modelClass, units] of [
    ['text-generation', ['input_token','output_token']], ['embedding', ['input_token','embedding']],
    ['workflow', ['pixel_frame']], ['image-generation', ['image','step']],
    ['video-generation', ['frame','video_second']], ['tts', ['audio_second','input_character']],
    ['stt', ['audio_second']], ['audio-generation', ['audio_second','input_character']],
    ['music-generation', ['audio_second','input_character']],
  ]) {
    const ctx = await market({ calibrated: false, modelClass, units });
    const first = Object.fromEntries(units.map((u,i) => [u, String((i+1)*100)]));
    const twice = Object.fromEntries(units.map((u,i) => [u, String((i+1)*200)]));
    await ctx.step(1, first);
    const up = await ctx.step(2, twice);
    assert.equal(up.record.market.activity_basis, 'relative_dimension_vector_v1', modelClass);
    assert.equal(up.momentum_bps, 20000, modelClass); assert.equal(up.frozen, false, modelClass);
    const down = await ctx.step(3, first);
    assert.ok(amount(down) < amount(up), modelClass);
  }
});

test('fixed work per second is invariant to epoch length, prices, and large exact counts', async () => {
  const ctx = await market();
  await ctx.step(1, { input_token: '9007199254740993000' }, { seconds: 3600 });
  const sameRate = await ctx.step(2, { input_token: '4503599627370496500' }, { seconds: 1800 });
  assert.equal(sameRate.momentum_bps, 10000); assert.equal(amount(sameRate), 1000000n);
});

test('successive falls reach the lower band and consecutive empty epochs keep decreasing', async () => {
  const ctx = await market();
  await ctx.step(1, { input_token: '1000' });
  let last;
  for (let epoch = 2; epoch < 50; epoch++) {
    last = await ctx.step(epoch, { input_token: (1000n * 4n**BigInt(epoch)).toString() });
    assert.ok(amount(last) <= 4000000n);
  }
  assert.equal(amount(last), 4000000n);
  for (let epoch = 50; epoch < 95; epoch++) {
    const next = await ctx.step(epoch, { input_token: (1000n * 4n**BigInt(98-epoch)).toString() });
    assert.ok(amount(next) >= 250000n);
    assert.ok(amount(last)-amount(next) <= amount(last)/10n);
    last = next;
  }
  assert.equal(amount(last), 250000n);
  const dormant = await market();
  await dormant.step(1, { input_token: '1000' });
  const zero = await dormant.step(2, {});
  const equalZero = await dormant.step(3, {});
  assert.equal(amount(zero), 900000n);
  assert.equal(amount(equalZero), 810000n);
  assert.equal(equalZero.momentum_bps, 0);
});

test('canonical work excludes billing baselines and unpriced injected dimensions', () => {
  const c = new MayhemContract({}, {});
  const body = { usage: { input_token: 120, output_token: 20, invented_work: 9999999 },
    billing_prior_usage: { input_token: 100, output_token: 10 },
    locked_rate_map: [{ unit: 'input_token', per_unit_au: '1', granularity: 1 },
      { unit: 'output_token', per_unit_au: '1', granularity: 1 }] };
  assert.deepEqual(c.incrementalSettledUsage(body), { input_token: '20', output_token: '10' });
  assert.ok(c.incrementalSettledUsage({ ...body, usage: { input_token: 99 } }) instanceof Error);
  const whole = { input_token: '30', output_token: '20' };
  const pages = c.addSettledUsage({ input_token: '10', output_token: '13' },
    { input_token: '20', output_token: '7' });
  assert.equal(c.calibratedActivityWork(whole, calibration), c.calibratedActivityWork(pages, calibration));
});

test('calibration changes bootstrap a new baseline and cannot create a price jump', async () => {
  const ctx = await market(); await ctx.step(1, { input_token: '1000' });
  const ref = (await ctx.storage.get(`modelref/${MODEL}`)).value;
  ref.activity_calibration.dimensions[0].units = '1';
  await ctx.storage.put(`modelref/${MODEL}`, ref);
  const reset = await ctx.step(2, { input_token: '1000' });
  assert.equal(reset.record.market.frozen_reason, 'activity_baseline_bootstrap');
  assert.equal(amount(reset), 1000000n);
});

test('price derivation hashes bind work, calibration, baseline and the result', async () => {
  const ctx = await market(); await ctx.step(1, { input_token: '1000' });
  const update = await ctx.step(2, { input_token: '2000' });
  const d = ctx.contract.priceDerivationFromMarketUpdate(update, { epoch: 2, at: 7200, epochSeconds: 3600, usageRoot: 'ab'.repeat(32) });
  const original = await ctx.contract.priceDerivationLeafHash(d);
  for (const change of [
    (v) => {v.usage.settled_usage.input_token = '2001';},
    (v) => {v.controller.previous_activity_rate = '1';},
    (v) => {v.controller.previous_activity_vector.input_token = '1';},
    (v) => {v.controller.calibration_hash = 'cd'.repeat(32);},
    (v) => {v.result_price.rate_map[0].per_unit_au = '1';},
  ]) { const tampered = structuredClone(d); change(tampered); assert.notEqual(await ctx.contract.priceDerivationLeafHash(tampered), original); }
});

test('hard-band migration removes unsafe pending records before their activation and preserves audit history', async () => {
  const ctx = await market(); const records = {};
  for (const [key, active, pending] of [['price_min_bps',2500,1],['price_max_bps',40000,1000000]]) {
    records[key] = { key, current: { value:active,ver:1,effective_at:0 },
      pending: { value:pending,ver:2,effective_at:86400 } };
    await ctx.storage.put(`params/${key}`,records[key]);
  }
  await ctx.storage.put('params/update/2',{ values:{price_min_bps:1,price_max_bps:1000000} });
  for (const at of [0,86400,9999999]) assert.deepEqual(await ctx.contract.activeParamsAt(at,['price_min_bps','price_max_bps']), {price_min_bps:2500,price_max_bps:40000});
  assert.ok(ctx.contract.validateParamValues({price_min_bps:1}) instanceof Error);
  assert.ok(ctx.contract.validateParamValues({price_max_bps:1000000}) instanceof Error);
  const outsider = await makeIdentity();
  const rejected = await execute(ctx.contract,ctx.storage,'migrateMarketPricing',{op:'migrate_market_pricing',at:0,markets:[]},outsider.publicKey,1);
  assert.match(rejected.message,/admin/i);
  const applied = await execute(ctx.contract,ctx.storage,'migrateMarketPricing',{op:'migrate_market_pricing',at:0,markets:[]},ctx.admin.publicKey,2);
  assert.equal(applied.repaired,2);
  for (const key of Object.keys(records)) assert.equal((await ctx.storage.get(`params/${key}`)).value.pending,null);
  assert.deepEqual((await ctx.storage.get('params/update/2')).value.values,{price_min_bps:1,price_max_bps:1000000});
  const repeated = await execute(ctx.contract,ctx.storage,'migrateMarketPricing',{op:'migrate_market_pricing',at:0,markets:[]},ctx.admin.publicKey,3);
  assert.equal(repeated.idempotent,true);
});

test('migration rejects partial epoch upgrades without modifying parameters', async () => {
  const ctx = await market();
  await ctx.storage.put('epoch/apply/state',{updated_epoch:1,pending_epoch:2,pending_next_page:1});
  const result = await execute(ctx.contract,ctx.storage,'migrateMarketPricing',{op:'migrate_market_pricing',at:0,markets:[]},ctx.admin.publicKey,1);
  assert.match(result.message,/completed epoch boundary/);
  assert.equal(await ctx.storage.get('market/activity/migration-v2'),null);
});


test('one-provider markets rise after bootstrap and fixed terms stay inside seed bands', async () => {
  const ctx = await market({ fixedTerms: true });
  await ctx.step(1, { input_token: '1000' }, { providers: 1 });
  let previous = 1000000n;
  for (let epoch = 2; epoch <= 40; epoch++) {
    const u = await ctx.step(epoch, { input_token: (1000n*4n**BigInt(epoch)).toString() }, { providers: 1 });
    assert.equal(u.frozen, false);
    const current = BigInt(u.record.per_req_au);
    assert.ok(current - previous <= previous/10n);
    assert.ok(current <= 4000000n);
    assert.ok(BigInt(u.record.min_session_au) <= 8000000n);
    previous = current;
  }
  assert.equal(previous, 4000000n);
  for (let epoch = 41; epoch <= 90; epoch++) {
    const u = await ctx.step(epoch, { input_token: epoch < 81 ? (1000n * 4n**BigInt(80-epoch)).toString() : '0' }, { providers: 1 });
    assert.ok(BigInt(u.record.per_req_au) >= 250000n);
    assert.ok(BigInt(u.record.min_session_au) >= 500000n);
    previous = BigInt(u.record.per_req_au);
  }
  assert.equal(previous, 250000n);
});

test('migration seeds active base and context markets before any new traffic', async () => {
  const ctx = await market();
  const base = 'ad'.repeat(32), baseModel = 'test/media';
  const rates = [{unit:'pixel_frame',per_unit_au:'1000',granularity:1}];
  await ctx.storage.put(`enclave/${base}`,{enclave_id:base,model_id:baseModel,model_class:'workflow',status:'active',caps:{}});
  await ctx.storage.put(`modelref/${baseModel}`,{model_id:baseModel,model_class:'workflow',rate_map:rates});
  await seedCurrentAdminPrice(ctx.storage,{enclaveId:base,modelId:baseModel,admin:ctx.admin.publicKey,rateMap:rates,ctxBracket:null});
  const markets=[{enclave_id:base},{enclave_id:ENCLAVE,ctx_bracket:'le8k',ctx_bracket_table_ver:1}];
  const migration=await execute(ctx.contract,ctx.storage,'migrateMarketPricing',{op:'migrate_market_pricing',at:0,markets},ctx.admin.publicKey,1);
  assert.equal(migration.market_count,2);
  ctx.contract.storage=ctx.storage;
  const empty=await ctx.contract.computeMarketPriceUpdates(new Map(),{epoch:1,at:3600,epochSeconds:3600,includeDormant:true,canonicalActivity:new Map()});
  assert.equal(empty.length,2);assert.ok(empty.every(u=>u.record.market.activity_initialized));
  const before=ctx.storage.snapshotBytes();
  const bad=await execute(ctx.contract,ctx.storage,'migrateMarketPricing',{op:'migrate_market_pricing',at:0,markets:[{enclave_id:'af'.repeat(32)}]},ctx.admin.publicKey,2);
  assert.match(bad.message,/not active/);assert.equal(ctx.storage.snapshotBytes(),before);
});

test('calibration must cover every priced workload axis with positive work', () => {
  const c=new MayhemContract({},{});
  const rates=[{unit:'input_token'},{unit:'output_token'}];
  assert.equal(c.validateActivityCalibration(calibration,'text-generation',rates),null);
  const partial=structuredClone(calibration);partial.dimensions.pop();
  assert.match(c.validateActivityCalibration(partial,'text-generation',rates).message,/every model reference/);
  const zero=structuredClone(calibration);zero.dimensions[0].work_us='0';
  assert.ok(c.validateActivityCalibration(zero,'text-generation',rates) instanceof Error);
  assert.match(c.validateActivityCalibration(calibration,'text-generation',[...rates,{unit:'cached_input_token'}]).message,/every model reference/);
});

test('activity index overflow fails before a price or settlement write', async () => {
  const ctx=await market();
  await ctx.storage.put('market/activity/index',Array.from({length:5000},(_,i)=>({enclave_id:'market-'+i})));
  const before=ctx.storage.snapshotBytes();
  const row={enclave_id:ENCLAVE,ctx_bracket:'le8k',ctx_bracket_table_ver:1,demand_au:'10',session_count:1,provider_count:1};
  const result=await ctx.contract.computeMarketPriceUpdates(ctx.contract.aggregateMarketUsageEntries([row]),{epoch:1,at:3600,epochSeconds:3600});
  assert.match(result.message,/capacity exceeded/);assert.equal(ctx.storage.snapshotBytes(),before);
});


test('admin can clear calibration explicitly while omission preserves it', async () => {
  const { contract, storage, admin, step } = await market();
  await step(1, { input_token: '1000', output_token: '100' });
  const ref = (await storage.get(`modelref/${MODEL}`)).value;
  const value = { op: 'set_model_ref', model_id: MODEL, model_class: ref.model_class, rate_map: ref.rate_map };
  const preserved = await execute(contract, storage, 'setModelRef', value, admin.publicKey, 301);
  assert.equal(preserved.ok, true, preserved.message);
  assert.deepEqual((await storage.get(`modelref/${MODEL}`)).value.activity_calibration, calibration);
  const cleared = await execute(contract, storage, 'setModelRef', { ...value, activity_calibration: null }, admin.publicKey, 302);
  assert.equal(cleared.ok, true, cleared.message);
  assert.equal((await storage.get(`modelref/${MODEL}`)).value.activity_calibration, undefined);
  contract.storage = storage; contract.address = admin.publicKey;
  const next = await step(2, { input_token: '9999', output_token: '9999' });
  assert.equal(next.record.market.activity_basis, 'relative_dimension_vector_v1');
  assert.equal(next.record.price_source, 'market_activity_hold');
});


test('immediately previous activity controls rise, fall and equality despite opposing EMA', async () => {
  for (const calibrated of [true, false]) {
    const ctx = await market({ calibrated });
    const counts = (n) => ({ input_token: String(n * 10), output_token: String(n) });
    await ctx.step(1, counts(100));
    const surge = await ctx.step(2, counts(400));
    const lower = await ctx.step(3, counts(300));
    assert.ok(amount(lower) < amount(surge), '300 falls from 400 despite exceeding EMA 175');
    assert.ok(lower.momentum_bps >= 7499 && lower.momentum_bps <= 7500);
    const equal = await ctx.step(4, counts(300));
    assert.equal(amount(equal), amount(lower), 'equal work holds despite lower EMA');
    const trough = await ctx.step(5, counts(10));
    const higher = await ctx.step(6, counts(20));
    assert.ok(amount(higher) > amount(trough), '20 rises from 10 despite remaining below EMA');
    assert.ok(higher.momentum_bps >= 20000 && higher.momentum_bps <= 20001);
    assert.deepEqual(higher.record.market.previous_activity_vector, trough.record.market.activity_vector);
    assert.equal(higher.record.market.previous_activity_rate, trough.record.market.activity_rate);
  }
});


test('initialized consecutive empty epochs decrease to the hard floor in both activity modes', async () => {
  for (const calibrated of [true, false]) {
    const ctx = await market({ calibrated, fixedTerms: true });
    await ctx.step(1, {}, { providers: 1 });
    let previous = 1000000n;
    for (let epoch = 2; epoch <= 30; epoch++) {
      const updates = await ctx.contract.computeMarketPriceUpdates(new Map(), {
        epoch, at: epoch * 3600, epochSeconds: 3600, canonicalActivity: new Map(), includeDormant: true,
      });
      assert.ok(!(updates instanceof Error), updates.message);
      const next = updates[0];
      await ctx.storage.put(next.schedule_key, next.schedule);
      assert.equal(next.record.market.active_supply, 0);
      assert.equal(next.record.market.session_count, 0);
      assert.equal(next.momentum_bps, 0);
      assert.ok(amount(next) >= 250000n);
      assert.ok(previous - amount(next) <= previous / 10n);
      if (previous > 250000n) assert.ok(amount(next) < previous);
      else assert.equal(amount(next), previous);
      assert.ok(BigInt(next.record.per_req_au) >= 250000n);
      assert.ok(BigInt(next.record.min_session_au) >= 500000n);
      previous = amount(next);
    }
    assert.equal(previous, 250000n);
  }
});
