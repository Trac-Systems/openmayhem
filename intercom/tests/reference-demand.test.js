import assert from 'node:assert/strict';
import test from 'node:test';
import {advanceDemand, demandObservation, demandTarget, scaleDemandPrice,
  DEMAND_PRECISION as Q} from '../contract/reference-demand.js';

const row = (epoch, count, unit = 'input_token', extra = {}) => ({epoch,
  epoch_seconds: 3600, usage: {settled_usage: {[unit]: String(count)}, session_count: count ? 1 : 0}, ...extra});
const step = (state, epoch, count, unit, extra) => advanceDemand(state, demandObservation(row(epoch, count, unit, extra)));
const ratio = (x) => Number(x[0]) / Number(x[1]);
const warm = (count = 100, unit) => {
  let state;
  for (let epoch = 1; epoch <= 72; epoch++) state = step(state, epoch, count, unit).state;
  return state;
};

test('reference uses past paid hourly units, starts at paid work, and ignores missing evidence', () => {
  let state = step(null, 1, 0).state;
  assert.equal(state.warmup.length, 0);
  state = step(state, 2, 100).state;
  state = advanceDemand(state, demandObservation({epoch: 3, epoch_seconds: 3600, usage: {session_count: 9}})).state;
  assert.equal(state.warmup.length, 1);
  for (let epoch = 4; epoch <= 74; epoch++) state = step(state, epoch, 100).state;
  assert.equal(state.reference.first_epoch, 2);
  assert.equal(state.reference.last_epoch, 74);
  assert.equal(state.last_target, null);
  assert.equal(state.history.length, 72);
  assert.equal(state.warmup.length, 0);
  const original = structuredClone(state.reference);
  state = step(state, 75, 10000000).state;
  assert.deepEqual(state.reference, original);
});

test('all unit types, elapsed speeds and capacities share the same response', () => {
  for (const unit of ['input_token', 'output_token', 'embedding', 'image', 'step',
    'megapixel_step', 'pixel_frame', 'frame', 'video_second', 'audio_second', 'input_character', 'compute']) {
    let state = warm(100, unit);
    for (let epoch = 73; epoch <= 144; epoch++) {
      const a = step(state, epoch, 50, unit);
      const b = step(state, epoch, 50, unit, {compute_ms: 1e12, capacity_slot_count: 1000, demand_au: '9999'});
      assert.deepEqual(a, b);
      state = a.state;
    }
    assert.equal(ratio(state.last_target.multiplier), 1.75, unit);
  }
});

test('historically demonstrated busy workload reaches and leaves both bounds', () => {
  let state = warm();
  let next = step(state, 73, 100);
  assert.equal(ratio(next.target.multiplier), 4);
  state = next.state;
  for (let epoch = 74; epoch <= 79; epoch++) {
    next = step(state, epoch, 0); state = next.state;
    if (epoch === 74) assert.equal(ratio(next.target.multiplier), 3.375);
  }
  assert.equal(ratio(next.target.multiplier), .25);
  next = step(state, 80, 1);
  assert.ok(ratio(next.target.multiplier) > .25);
});

test('sustained 5% demand decline settles at 370.75%, with no previous-price memory', () => {
  let state = warm();
  for (let epoch = 73; epoch <= 144; epoch++) state = step(state, epoch, 95).state;
  const m = state.last_target.multiplier;
  assert.equal(ratio(m), 3.7075);
  assert.equal(scaleDemandPrice('1000000', m), '3707500');
  // Price scaling takes only the immutable currency reference and new target.
  assert.equal(scaleDemandPrice('0', m), '0');
  assert.equal(scaleDemandPrice('1', ['1', '4']), '1');
});

test('unknown evidence and new axes hold without mutating history or resetting the reference', () => {
  const state = warm();
  for (const units of [null, {new_axis: ['1', '1']}]) {
    const r = advanceDemand(state, {epoch: 73, units});
    assert.equal(r.target, null);
    assert.deepEqual(r.state.history, state.history);
    assert.deepEqual(r.state.reference, state.reference);
  }
  const emptyPaid = demandObservation(row(73, 0, 'input_token', {usage: {session_count: 2, settled_usage: {}}}));
  assert.equal(emptyPaid.units, null);
  assert.deepEqual(demandObservation(row(73, 0)).units, {});
});

test('burst clipping bounds memory, restart is exact, duplicate conflicts cannot alter evidence', () => {
  let state = warm();
  for (let epoch = 73; epoch < 300; epoch++) {
    const result = step(state, epoch, epoch % 19 ? epoch % 101 : 10 ** 15);
    assert.deepEqual(step(JSON.parse(JSON.stringify(state)), epoch, epoch % 19 ? epoch % 101 : 10 ** 15), result);
    state = result.state;
    assert.equal(state.history.length, 72);
    const price = ratio(result.target.multiplier);
    assert.ok(price >= .25 && price <= 4);
    const duplicate = step(state, epoch, epoch % 19 ? epoch % 101 : 10 ** 15);
    assert.equal(duplicate.idempotent, true);
    assert.deepEqual(duplicate.state, state);
    assert.throws(() => step(state, epoch, 19999999), /Conflicting/);
  }
  assert.ok(state.history.every((x) => BigInt(x) <= Q));
});

test('hour normalization and equivalent unit scaling preserve reference and demand', () => {
  let stateA, stateB;
  for (let epoch = 1; epoch <= 200; epoch++) {
    const count = 100 + epoch % 7;
    stateA = step(stateA, epoch, count).state;
    stateB = step(stateB, epoch, count * 2, 'input_token', {epoch_seconds: 7200}).state;
  }
  assert.deepEqual(stateA, stateB);
});

test('no hidden 100% attractor and no demand overhang after six zero observations', () => {
  assert.equal(ratio(demandTarget(Array(72).fill(String(Q / 10n))).multiplier), .625);
  assert.equal(ratio(demandTarget(Array(72).fill(String(Q / 2n))).multiplier), 1.75);
  const history = Array(65).fill('0').concat(String(Q), Array(6).fill('0'));
  assert.equal(ratio(demandTarget(history).multiplier), .25);
});
