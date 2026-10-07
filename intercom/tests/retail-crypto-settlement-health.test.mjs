import assert from 'node:assert/strict';
import test from 'node:test';
import { readSettlementHealth } from '../scripts/retail-crypto-settlement-health.mjs';

const USD = 10n ** 18n;
const HASH = 'a'.repeat(64);
function fixture(rail = 'tnk', count = 2) {
  const epoch = 20_001;
  const records = new Map([
    ['epoch/apply/state', { updated_epoch: epoch, last_apply_hash: HASH, last_settlement_unix: 200 }],
    [`epoch/apply-anchor/${epoch}`, { type: 'epoch_apply_anchor', epoch, apply_hash: HASH, settlement_unix: 200 }],
    ['params/payout_min_au', { key: 'payout_min_au', current: { value: String(USD) }, pending: null }],
  ]);
  const entries = Array.from({ length: count }, (_, i) => ({
    provider: (i + 1).toString(16).padStart(64, '0'), payout_revision: 'b'.repeat(64),
  }));
  records.set(`payout/liability-index/${rail}`, {
    type: 'provider_payout_liability_index', rail, updated_epoch: epoch, entries,
  });
  const keys = entries.map(({ provider, payout_revision }) => {
    const key = `payout/liability/${rail}/${provider}/${payout_revision}`;
    records.set(key, { type: 'provider_payout_liability', rail, provider, revision: payout_revision,
      updated_epoch: epoch, total_au: String(USD / 2n), held_au: '0', paid_cum_au: '0' });
    return key;
  });
  const calls = [];
  let inFlight = 0, maxInFlight = 0;
  const read = async (key, options) => {
    calls.push({ key, options });
    assert.equal(options?.prefix, undefined, 'history prefix scans are forbidden');
    if (key !== 'epoch/apply/state') assert.deepEqual(options, { signedLength: 56789 });
    inFlight++; maxInFlight = Math.max(maxInFlight, inFlight);
    await new Promise((resolve) => setImmediate(resolve));
    inFlight--;
    return { key, value: structuredClone(records.get(key) ?? null), confirmed: true, signed_length: 56789 };
  };
  return { rail, epoch, records, keys, calls, read, maxInFlight: () => maxInFlight,
    run: () => readSettlementHealth({ read, rail, maxEpochLag: 2, nowUnix: 200 }) };
}

for (const rail of ['tnk', 'tap']) {
  test(`${rail}: below-minimum liabilities are backed but do not disable admission`, async () => {
    const f = fixture(rail, 4);
    const result = await f.run();
    assert.equal(result.unsettledAu, 2n * USD);
    assert.equal(result.payableAu, 2n * USD);
    assert.equal(result.payoutStatus, 'current');
    assert.equal(f.calls.length, 4 + 4 + 3);
  });

  test(`${rail}: old history cannot overflow or extend the health query`, async () => {
    const f = fixture(rail);
    for (let i = 1; i <= 2_000; i++) {
      f.records.set(`settle/targeted/${rail}/${i}/output/${HASH}`, { type: 'old_output' });
      f.records.set(`epoch/apply-anchor/${i}`, { epoch: i });
    }
    f.records.set(`settle/targeted/${rail}/${f.epoch}`, {
      type: 'targeted_payout_epoch_close', rail, epoch: f.epoch, outcome: 'carry',
    });
    const result = await f.run();
    assert.equal(result.lastSettledEpoch, f.epoch);
    assert.equal(result.lag, 0);
    assert.equal(result.payoutStatus, 'current');
    assert.equal(f.calls.length, 7);
  });

  test(`${rail}: genuinely overdue payable liability remains blocked`, async () => {
    const f = fixture(rail);
    f.records.get(f.keys[0]).total_au = String(USD);
    assert.equal((await f.run()).payoutStatus, 'lagging');
    f.records.set(`settle/targeted/${rail}/${f.epoch - 2}`, {
      type: `targeted_${rail}_settlement`, epoch: f.epoch - 2,
    });
    const result = await f.run();
    assert.equal(result.payoutStatus, 'pending');
    assert.equal(result.lag, 2);
  });
}

test('held and already paid money is excluded from payable, retained backing is exact', async () => {
  const f = fixture();
  Object.assign(f.records.get(f.keys[0]), { total_au: String(10n * USD), held_au: String(8n * USD), paid_cum_au: String(USD) });
  const result = await f.run();
  assert.equal(result.unsettledAu, 9n * USD + USD / 2n);
  assert.equal(result.heldAu, 8n * USD);
  assert.equal(result.payableAu, USD + USD / 2n);
  assert.equal(result.payoutStatus, 'lagging');
});

test('canonical parameter activates at its effective time, including zero minimum', async () => {
  const f = fixture();
  const param = f.records.get('params/payout_min_au');
  param.pending = { value: '0', effective_at: 201 };
  assert.equal((await f.run()).payoutStatus, 'current');
  param.pending.effective_at = 200;
  assert.equal((await f.run()).payoutStatus, 'lagging');
  for (const key of f.keys) f.records.get(key).held_au = f.records.get(key).total_au;
  assert.equal((await f.run()).payoutStatus, 'current');
});

test('all reads share one confirmed checkout and concurrency stays bounded', async () => {
  const f = fixture('tnk', 1_005);
  await f.run();
  assert.equal(f.calls.length, 4 + 1_005 + 3);
  assert.ok(f.maxInFlight() <= 4);
});

test('genesis with no liability index is healthy; missing funded state is not', async () => {
  const f = fixture();
  f.records.set('epoch/apply/state', { updated_epoch: 0, last_settlement_unix: null });
  f.records.delete('payout/liability-index/tnk');
  assert.equal((await f.run()).payoutStatus, 'current');
  f.records.delete('epoch/apply/state');
  await assert.rejects(f.run, /settlement_state_invalid/);
});

test('missing, duplicate, unsorted or mismatched indexed liabilities fail closed', async () => {
  for (const mutate of [
    (f) => f.records.delete(f.keys[0]),
    (f) => f.records.get(f.keys[0]).provider = HASH,
    (f) => f.records.get(f.keys[0]).rail = 'tap',
    (f) => f.records.get(f.keys[0]).total_au = '-1',
    (f) => f.records.get(f.keys[0]).paid_cum_au = String(USD),
    (f) => f.records.get('payout/liability-index/tnk').entries.reverse(),
    (f) => f.records.get('payout/liability-index/tnk').entries.push(f.records.get('payout/liability-index/tnk').entries[0]),
    (f) => f.records.get(`epoch/apply-anchor/${f.epoch}`).apply_hash = 'c'.repeat(64),
    (f) => f.records.get('params/payout_min_au').pending = { value: '0', effective_at: -1 },
  ]) {
    const f = fixture(); mutate(f);
    await assert.rejects(f.run);
  }
});

test('changed height, unconfirmed evidence, missing value and wrong key fail closed', async () => {
  for (const change of [{ signed_length: 56790 }, { confirmed: false }, { key: 'wrong' }, { value: undefined }]) {
    const f = fixture();
    await assert.rejects(readSettlementHealth({ rail: 'tnk', maxEpochLag: 2, read: async (key, options) => {
      const result = await f.read(key, options);
      return key === 'payout/liability-index/tnk' ? { ...result, ...change } : result;
    }}));
  }
});

test('a settlement for the wrong rail or epoch cannot make overdue funds healthy', async () => {
  for (const close of [
    { type: 'targeted_payout_epoch_close', epoch: 20_001, rail: 'tap' },
    { type: 'targeted_payout_epoch_close', epoch: 20_000, rail: 'tnk' },
    { type: 'targeted_tap_settlement', epoch: 20_001 },
  ]) {
    const f = fixture(); f.records.set(`settle/targeted/tnk/${f.epoch}`, close);
    await assert.rejects(f.run, /settlement_state_invalid/);
  }
});
