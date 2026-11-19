import test from 'node:test';
import assert from 'node:assert/strict';
import { resolveTapLiabilitySweep } from '../scripts/tap-liability-sweep.mjs';

const DOLLAR = 10n ** 18n;
const PROVIDER = 'b'.repeat(64);
const ADMIN = 'a'.repeat(64);
const HASH = 'c'.repeat(64);
const EPOCH = 1908;
const AT = 1800000000;
const PIN = 48123;
const POOL = '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28';
const TOKEN = '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454';
const PREFIX = 'payout/liability/tap/';
const APPLY = 'epoch/apply/state';
const ANCHOR = `epoch/apply-anchor/${EPOCH}`;
const EARNING = `earn/tap/${PROVIDER}`;
const DISPUTE = `disp/provider-open/${PROVIDER}`;
const PROBE = `probe/pass/${PROVIDER}/167`;
const id = (n) => n.toString(16).padStart(64, '0');

function fixture() {
  const state = new Map([
    ['admin', ADMIN],
    ['payments/current', {
      set_by: ADMIN, set_by_role: 'admin', ver: 3, denom: 'au_usd', rails: ['tap'],
      tnk: { network: 'mainnet' }, tap: { chain_id: 1, token_address: TOKEN, pool_address: POOL },
    }],
    [APPLY, { updated_epoch: EPOCH, last_apply_hash: HASH, pending_epoch: null,
      updated_at: 'epoch-tx', last_settlement_unix: AT }],
    [ANCHOR, { type: 'epoch_apply_anchor', epoch: EPOCH, apply_hash: HASH, applied_at: 'epoch-tx',
      settlement_unix: AT }],
  ]);
  const reads = [];
  const pages = [];
  function param(key, value, pending = null) {
    state.set(`params/${key}`, { key, current: { value, effective_at: 0, ver: 1 }, pending });
  }
  function add(n = 1, { provider = PROVIDER, held = DOLLAR, free = 0n, paid = 13n,
    buckets = held ? [{ epoch: 167, au: String(held), locked_epochs: 168 }] : [] } = {}) {
    const revision = id(n);
    const key = `${PREFIX}${provider}/${revision}`;
    const bindingKey = `payout/binding/tap/${provider}/${revision}`;
    const target = `0x${revision.slice(-40)}`;
    const binding = { type: 'provider_payout_binding', rail: 'tap', provider, revision,
      currency: null, chain_id: 1, target, verified: true, network: 'mainnet',
      payment_config_version: 1, activation_epoch: 100, expires_after_epoch: 120 };
    const row = { type: 'provider_payout_liability', provider, rail: 'tap', revision,
      currency: null, target, chain_id: 1, updated_epoch: 167, total_au: String(held + free + paid),
      held_au: String(held), paid_cum_au: String(paid), holdbacks: structuredClone(buckets) };
    state.set(bindingKey, binding);
    state.set(key, row);
    const earningKey = `earn/tap/${provider}`;
    if (!state.has(earningKey)) {
      state.set(`prov/${provider}`, { provider, status: 'active' });
      state.set(earningKey, { rail: 'tap', provider, denom: 'au_usd', total_au: '0', paid_cum_au: '0',
        held_au: '0', updated_epoch: 167, holdbacks: [], chain_id: 1, pool_address: POOL });
    }
    const earning = state.get(earningKey);
    for (const k of ['total_au', 'held_au', 'paid_cum_au']) {
      earning[k] = String(BigInt(earning[k]) + BigInt(row[k]));
    }
    earning.holdbacks.push(...structuredClone(buckets));
    return { key, binding, row, revision, target, bindingKey };
  }
  // In-memory implementation of the exclusive prefix and exact-key RPC wire format.
  // Unknown keys (including history, pointers or other prefix reads) are test failures.
  function run(options = {}, alter = () => {}) {
    return resolveTapLiabilitySweep({ peerRpcUrl: 'https://fixture.invalid/v1', epoch: EPOCH,
      epochApplyHash: HASH, signedLength: PIN, at: AT, ...options,
      fetchImpl: async (url) => {
        assert.equal(url.pathname, '/v1/state');
        const q = url.searchParams;
        assert.equal(q.get('confirmed'), 'true');
        assert.equal(q.get('signed_length'), String(PIN));
        const body = { confirmed: true, signed_length: PIN };
        const key = q.get('key');
        if (key !== null) {
          reads.push(key);
          assert(!q.has('prefix'));
          assert(state.has(key) || /^(?:params\/|payout\/preparation-tail\/tap\/|disp\/provider-open\/|probe\/pass\/|auditor\/|epoch\/apply-anchor\/)/.test(key),
            `Unexpected key: ${key}`);
          Object.assign(body, { key, value: state.get(key) ?? null });
        } else {
          const limit = Number(q.get('limit'));
          const after = q.get('after');
          pages.push({ limit, after });
          assert.equal(q.get('prefix'), PREFIX);
          assert(Number.isSafeInteger(limit) && limit >= 1 && limit <= 32);
          const keys = [...state.keys()].filter(k => k.startsWith(PREFIX) && (after === null || k > after)).sort();
          const values = keys.slice(0, limit).map(k => ({ key: k, value: state.get(k) }));
          const truncated = keys.length > limit;
          Object.assign(body, { prefix: PREFIX, values, truncated, next_cursor: truncated ? values.at(-1).key : null });
        }
        return alter(body, key) ?? Response.json(body);
      },
    });
  }
  return { state, reads, pages, add, param, run };
}

test('refreshes an old verified revision at epoch 1908 from stored zero payable to exactly one dollar', async () => {
  const f = fixture();
  const { revision, target } = f.add();
  f.state.set(`payout/current/tap/${PROVIDER}`, { current_revision: id(2) });
  f.state.get(`prov/${PROVIDER}`).status = 'banned';
  const result = await f.run({ total_au: '99999999', liabilities: [] });
  assert.deepEqual(result, {
    signed_length: PIN, after: null, next_after: null, payout_min_au: String(DOLLAR), fee_bps: 1500,
    max_outputs: 5000, liabilities: [{ provider: PROVIDER, rail: 'tap', payout_revision: revision,
      target, chain_id: 1, total_au: String(DOLLAR + 13n), held_au: '0', paid_cum_au: '13',
      aggregate_paid_cum_au: '13', updated_epoch: 167 }],
  });
  assert.deepEqual(f.pages, [{ limit: 32, after: null }]);
  assert(!f.reads.some(k => k.startsWith('payout/current/') || k.startsWith('probe/')));
});

test('uses each bucket lock and legacy fallback, and keeps held funds during an open dispute', async () => {
  const f = fixture();
  const held = 2n * DOLLAR;
  f.add(1, { held, free: DOLLAR, buckets: [
    { epoch: 160, au: String(DOLLAR) },
    { epoch: 167, au: String(DOLLAR), locked_epochs: EPOCH - 167 + 1 },
  ] });
  f.param('holdback_epochs', EPOCH - 160);
  assert.equal((await f.run()).liabilities[0].held_au, String(DOLLAR));
  f.state.set(DISPUTE, { provider: PROVIDER, count: 1 });
  assert.equal((await f.run()).liabilities[0].held_au, String(held));
  const g = fixture();
  const { row } = g.add();
  delete row.holdbacks;
  delete g.state.get(EARNING).holdbacks;
  g.state.set(DISPUTE, { provider: PROVIDER, count: 1 });
  assert.deepEqual((await g.run()).liabilities, []);
  g.state.get(DISPUTE).count = 0;
  assert.equal((await g.run()).liabilities[0].held_au, '0');
});

test('only active distinct auditors release the probe portion of a matured bucket', async () => {
  const f = fixture();
  const a = id(90), b = id(91);
  f.add(1, { held: 2n * DOLLAR });
  f.param('canary_probe_holdback_bps', 5000);
  f.state.set(PROBE, { provider: PROVIDER, epoch: 167, auditors: [a, b], pass_count: 2 });
  f.state.set(`auditor/${a}`, { auditor: a, status: 'active' });
  f.state.set(`auditor/${b}`, { auditor: b, status: 'slashed' });
  assert.equal((await f.run()).liabilities[0].held_au, String(DOLLAR));
  assert.equal(f.reads.filter(k => k === PROBE).length, 1);
  // A previously gated bucket must retain its full remainder on a later refresh.
  const { row } = f.add(2, { free: DOLLAR, held: DOLLAR,
    buckets: [{ epoch: 167, au: String(DOLLAR), probe_gate: true }] });
  assert.equal((await f.run()).liabilities[1].held_au, String(DOLLAR));
  f.state.get(`auditor/${b}`).status = 'active';
  assert.deepEqual((await f.run()).liabilities.map(r => [r.payout_revision, r.held_au]),
    [[id(1), '0'], [row.revision, '0']]);
  f.state.get(PROBE).auditors = [a, a];
  await assert.rejects(f.run(), /probe pass/);
});

test('refreshes distinct aggregate buckets and covers the sum across revisions with one paid watermark', async () => {
  const f = fixture();
  f.add(1, { paid: 8n });
  f.add(2, { paid: 12n });
  assert.deepEqual((await f.run()).liabilities.map(r => [r.paid_cum_au, r.aggregate_paid_cum_au]),
    [['8', '20'], ['12', '20']]);
  assert.equal(f.reads.filter(k => k === EARNING).length, 1);
  const aggregate = f.state.get(EARNING);
  aggregate.holdbacks[1].locked_epochs = 2000;
  await assert.rejects(f.run(), /exceed refreshed aggregate payable/);
  aggregate.holdbacks[1].locked_epochs = 0;
  aggregate.paid_cum_au = '19'; // Each individual revision fits, their historic paid amounts do not.
  await assert.rejects(f.run(), /aggregate paid watermark/);
});

test('aggregate probe gating is evaluated for the aggregate bucket epoch independently', async () => {
  const f = fixture();
  f.add();
  f.param('canary_probe_holdback_bps', 10000);
  const a = id(90), b = id(91);
  f.state.set(PROBE, { auditors: [a, b], pass_count: 2 });
  for (const auditor of [a, b]) f.state.set(`auditor/${auditor}`, { auditor, status: 'active' });
  f.state.get(EARNING).holdbacks[0].epoch = 166;
  await assert.rejects(f.run(), /exceed refreshed aggregate payable/);
  assert(f.reads.includes(`probe/pass/${PROVIDER}/166`));
});

test('keeps aggregate spending and paid watermarks separate for different providers', async () => {
  const f = fixture();
  const other = 'd'.repeat(64);
  f.add(1, { provider: other, paid: 77n });
  f.add(1, { paid: 5n });
  assert.deepEqual((await f.run()).liabilities.map(r => [r.provider, r.aggregate_paid_cum_au]),
    [[PROVIDER, '5'], [other, '77']]);
  f.state.get(EARNING).total_au = String(10n * DOLLAR);
  f.state.get(`earn/tap/${other}`).holdbacks[0].locked_epochs = 2000;
  await assert.rejects(f.run(), /exceed refreshed aggregate payable/);
});

test('selects active parameters at the supplied timestamp and rotates past filtered rows', async () => {
  const f = fixture();
  const first = f.add(1);
  const second = f.add(2, { held: DOLLAR - 1n });
  const third = f.add(3);
  f.param('max_tap_settlement_outputs', 1, { value: 2, effective_at: AT, ver: 2 });
  f.param('payout_min_au', String(2n * DOLLAR), { value: String(DOLLAR), effective_at: AT, ver: 2 });
  f.param('fee_bps', 1500, { value: 1400, effective_at: AT + 1, ver: 2 });

  await assert.rejects(f.run({ at: AT - 1 }), /liability snapshot differs/);
  f.state.get(APPLY).last_settlement_unix = f.state.get(ANCHOR).settlement_unix = AT - 1;
  const before = await f.run({ at: AT - 1 });
  assert.equal(before.max_outputs, 1);
  assert.deepEqual(before.liabilities, []);
  assert.equal(before.next_after, first.key);
  f.state.get(APPLY).last_settlement_unix = f.state.get(ANCHOR).settlement_unix = AT;
  const start = await f.run();
  assert.deepEqual(start.liabilities.map(r => r.payout_revision), [first.revision]);
  assert.equal(start.max_outputs, 2);
  assert.equal(start.next_after, second.key);
  const end = await f.run({ after: start.next_after });
  assert.equal(end.after, second.key);
  assert.equal(end.next_after, null);
  assert.deepEqual(end.liabilities.map(r => r.payout_revision), [third.revision]);
  assert.deepEqual(f.pages, [{ limit: 1, after: null }, { limit: 2, after: null }, { limit: 2, after: second.key }]);
  f.state.get(APPLY).last_settlement_unix = f.state.get(ANCHOR).settlement_unix = AT + 1;
  await assert.rejects(f.run({ at: AT + 1 }), /fee schedule/);
});

test('caps large active output limits at 32 scan rows and handles a full terminal or empty page', async () => {
  const f = fixture();
  for (let n = 64; n >= 1; n--) f.add(n, { held: n <= 32 ? 0n : DOLLAR, paid: 0n });
  f.param('payout_min_au', '0');
  const first = await f.run();
  assert.equal(first.next_after, `${PREFIX}${PROVIDER}/${id(32)}`);
  assert.deepEqual(first.liabilities, []);
  const end = await f.run({ after: first.next_after });
  assert.equal(end.next_after, null);
  assert.deepEqual(end.liabilities.map(r => r.payout_revision), Array.from({ length: 32 }, (_, i) => id(33 + i)));
  const empty = await f.run({ after: `${PREFIX}${PROVIDER}/${id(64)}` });
  assert.deepEqual(empty.liabilities, []);
  assert.equal(empty.next_after, null);
});

test('binds the requested current or historical applied epoch, independent of the latest hash', async () => {
  const f = fixture();
  f.add();
  const head = f.state.get(APPLY);
  head.updated_epoch++;
  head.last_apply_hash = id(500);
  assert.equal((await f.run()).liabilities.length, 1);
  await assert.rejects(f.run({ epochApplyHash: head.last_apply_hash }), /apply hash mismatch/);
  f.state.delete(ANCHOR);
  await assert.rejects(f.run(), /Missing epoch apply anchor/);
  head.updated_epoch = EPOCH;
  head.last_apply_hash = HASH;
  assert.equal((await f.run()).liabilities.length, 1);
  head.pending_epoch = EPOCH + 1;
  await assert.rejects(f.run(), /Incomplete/);
  head.pending_epoch = null;
  head.updated_epoch--;
  await assert.rejects(f.run(), /future/);
});

test('rejects malformed, stale or substituted RPC responses', async () => {
  const f = fixture();
  const { bindingKey } = f.add();
  f.param('canary_probe_holdback_bps', 5000);
  for (const key of [ANCHOR, 'params/fee_bps', null, bindingKey, DISPUTE, PROBE, EARNING]) {
    await assert.rejects(f.run({}, (body, requested) => {
      if (requested === key) body.signed_length--;
    }), /signed length/);
  }
  for (const mutate of [
    body => { body.key = 'different'; }, body => { body.confirmed = false; },
    body => { delete body.value; },
  ]) {
    await assert.rejects(f.run({}, (body, key) => { if (key === DISPUTE) mutate(body); }), /pinned|unconfirmed/);
  }
  await assert.rejects(f.run({}, () => new Response('{')), SyntaxError);
  await assert.rejects(f.run({}, () => new Response(null, { status: 503 })), /HTTP 503/);
  await assert.rejects(f.run({}, body => { body.error = 'checkout unavailable'; }), /contains an error/);
  assert.deepEqual(await f.run({}, body => ({ ok: true, json: async () => body })), await f.run());
});

test('requires an explicit safe pin, epoch, hash and preparation time before the first read', async () => {
  const f = fixture();
  for (const options of [{ signedLength: undefined }, { signedLength: 0 }, { signedLength: '48123' },
    { epoch: 0 }, { epochApplyHash: 'invalid' }, { at: Number.MAX_SAFE_INTEGER + 1 }, { at: -1 }]) {
    await assert.rejects(f.run(options), /pinned TAP sweep inputs/);
  }
  assert.deepEqual(f.reads, []);
  assert.deepEqual(f.pages, []);
});

test('rejects cursor jumps, duplicates, out-of-order keys and inconsistent truncation', async () => {
  const f = fixture();
  f.add(1); f.add(2); f.add(3);
  f.param('max_tap_settlement_outputs', 2);
  for (const mutate of [
    p => { p.next_cursor = `${PREFIX}${PROVIDER}/${id(3)}`; },
    p => { p.truncated = false; }, p => { p.next_cursor = null; },
    p => { p.values.pop(); p.next_cursor = p.values[0].key; },
    p => { p.values.push(p.values[0]); }, p => { p.values.reverse(); },
    p => { p.values[1] = p.values[0]; }, p => { p.prefix = 'earn/tap/'; },
    p => { p.values[0].key = 'wrong'; }, p => { delete p.truncated; },
  ]) {
    await assert.rejects(f.run({}, (body, key) => { if (key === null) mutate(body); }), /page|key|cursor/);
  }
  const cursor = `${PREFIX}${PROVIDER}/${id(1)}`;
  await assert.rejects(f.run({ after: cursor }, (body, key) => {
    if (key === null) body.values[0].key = cursor;
  }), /advancing/);
  await assert.rejects(f.run({ after: 'earn/tap/' }), /after cursor/);
});

test('rejects inconsistent financial identities, records, schedules and contract gate errors', async () => {
  const cases = [
    [f => { f.state.get('payments/current').tap.token_address = POOL; }, /payment scope/],
    [f => { f.state.get('payments/current').tap.pool_address = TOKEN; }, /payment scope/],
    [f => { f.state.get('payments/current').set_by = id(1); }, /payment scope/],
    [f => { f.state.get(EARNING).pool_address = TOKEN; }, /aggregate scope/],
    [f => { f.state.set(EARNING, null); }, /earnings invariant/],
    [f => { f.state.get(EARNING).paid_cum_au = '012'; }, /earnings invariant/],
    [(f, r) => { r.binding.activation_epoch = EPOCH + 1; }, /binding/],
    [(f, r) => { delete r.binding.activation_epoch; }, /binding/],
    [(f, r) => { r.binding.verified = false; }, /binding/],
    [(f, r) => { r.binding.target = TOKEN; }, /binding/],
    [(f, r) => { r.binding.chain_id = 2; }, /binding/],
    [(f, r) => { r.binding.network = 'testnet1'; }, /binding/],
    [(f, r) => { r.row.revision = id(2); }, /revision invariant/],
    [(f, r) => { r.row.held_au = '1'; }, /conservation invariant/],
    [(f, r) => { r.row.holdbacks[0].locked_epochs = -1; }, /monotonic epoch invariant/],
    [(f, r) => { r.row.updated_epoch = EPOCH + 1; }, /updated after/],
    [f => { f.state.get(`prov/${PROVIDER}`).status = 'pending'; }, /provider status/],
    [f => { f.state.get(`prov/${PROVIDER}`).probation = { successful_sessions: -1 }; }, /probation state/],
    [f => { f.state.set(DISPUTE, {}); }, /dispute count/],
    [f => { f.state.set(DISPUTE, { count: -1 }); }, /dispute count/],
    [f => { f.param('max_tap_settlement_outputs', 0); }, /out of range/],
    [f => { f.param('max_tap_settlement_outputs', '2'); }, /integer/],
    [f => { f.param('canary_probe_holdback_bps', -1); }, /out of range/],
    [f => { f.param('canary_probe_holdback_bps', 1); f.param('canary_probe_release_min_passes', 1); }, /threshold/],
    [f => { f.param('payout_min_au', '0'); f.state.get('params/payout_min_au').current.effective_at = AT + 1; },
      /Missing historical TAP parameter/],
    [f => { f.state.get(ANCHOR).epoch--; }, /anchor is invalid/],
    [f => { f.state.get(APPLY).last_apply_hash = id(5); }, /apply hash mismatch/],
  ];
  for (const [mutate, error] of cases) {
    const f = fixture();
    mutate(f, f.add());
    await assert.rejects(f.run(), error);
  }
});

test('bounds probe fetch fan-out and streamed response bytes before returning any liabilities', async () => {
  const f = fixture();
  f.add();
  f.param('canary_probe_holdback_bps', 5000);
  const auditors = Array.from({ length: 600 }, (_, n) => id(n));
  f.state.set(PROBE, { auditors, pass_count: auditors.length });
  await assert.rejects(f.run(), /read limit/);
  assert(f.reads.length < auditors.length);
  await assert.rejects(f.run({}, () => new Response(new Uint8Array(8 * 1024 * 1024 + 1))), /byte limit/);
});

test('a reserved provider and an unverified binding leave other TAP entitlements payable', async () => {
  const f = fixture();
  const b = 'd'.repeat(64), c = 'e'.repeat(64);
  const aRow = f.add(1);
  f.add(1, {provider:b});
  const cRow = f.add(1, {provider:c});
  cRow.binding.verified = false;
  const tail = 'payout/preparation-tail/tap/' + PROVIDER;
  const op = id(123);
  f.state.set(tail, {economic_op_id:op, consumed:false, paid_cum_au_before:'13', paid_cum_au_after:'16'});
  const record = {type:'targeted_payout_preparation', kind:'liability', economic_op_id:op, consumed:false,
    rail:'tap', liability:{provider:PROVIDER,payout_revision:aRow.revision,aggregate_paid_cum_au_before:'13',paid_au:'3'}};
  f.state.set('payout/preparation/tap/' + op, record);
  const page = await f.run({schemaVersion:2});
  assert.deepEqual(page.liabilities.map(row => row.provider), [b]);
  assert.deepEqual(page.deferred, [
    {provider:PROVIDER,payout_revision:aRow.revision,reason:'outstanding_preparation',economic_op_id:op},
    {provider:c,payout_revision:cRow.revision,reason:'unverified_binding'}
  ]);
  record.liability.paid_au = '4';
  await assert.rejects(f.run({schemaVersion:2}), /matching outstanding liability/);
  f.state.set(tail, null);
  cRow.binding.verified = true;
  const next = await f.run({schemaVersion:2});
  assert.deepEqual(next.liabilities.map(row => row.provider), [PROVIDER,b,c]);
  assert.deepEqual(next.deferred, []);
});
