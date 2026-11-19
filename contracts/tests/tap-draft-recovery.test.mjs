import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { ZeroHash } from 'ethers';
import { advanceSweep, loadCursor, prepareSweep, validateQueuedSweep } from '../scripts/prepare-tap-sweep-bundle.mjs';
import { digest, handoffFile, journalTapPreparation, nextRefunds } from '../scripts/tap-draft-files.mjs';
import { recoverDraft } from '../scripts/tap-draft-recovery.mjs';
import { confirmedTapContract, preflightTapSweepPreparations } from '../scripts/tap-liability-sweep.mjs';
import { buildCanonicalTapPreparationPlan, guardianPreSignReport, projectTapRefunds, rollTapSettlement,
  tapRateRecordToLock } from '../scripts/tap-settlement-roller.mjs';
import { distribution } from '../scripts/merkle.mjs';
import { oracleRecord } from './helpers/tap-pricing.mjs';

const hash = n => n.toString(16).padStart(64, '0');
const addr = n => '0x' + n.toString(16).padStart(40, '0');
const scope = { chain_id: 1, pool_address: '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28',
  token_address: '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454', payment_config_ver: 1 };
const admin = hash(91);
const rateValue = 2n * 10n ** 18n;
const read = file => JSON.parse(fs.readFileSync(file));
const write = (file,obj) => fs.writeFileSync(file, JSON.stringify(obj));
const liabilityKey = i => 'payout/liability/tap/' + hash(i) + '/' + hash(i);
const emptyPending = () => [ZeroHash, 0n, 0n, 0n, 0n, 0n, 0n];

function fixture(t) {
  const spool = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-draft-test-'));
  t.after(() => fs.rmSync(spool, { recursive: true, force: true }));
  for (const d of ['working', 'processed', 'ready', 'failed']) fs.mkdirSync(path.join(spool, d));
  const fee = { ver: 1, effective_at: 0, value: 1500 };
  const state = new Map([
    ['admin', admin], ['params/fee_bps', { key: 'fee_bps', current: fee, pending: null }],
    ['params/payout_min_au', { key: 'payout_min_au', current: { ...fee, value: '1' }, pending: null }],
    ['payments/current', { set_by: admin, set_by_role: 'admin', ver: 1, denom: 'au_usd',
      rails: ['tap'], tnk: { network: 'mainnet' }, tap: { chain_id: 1, pool_address: scope.pool_address,
        token_address: scope.token_address } }],
  ]);
  for (const [i, amount] of [[1, '101'], [2, '12']]) {
    const row = { type: 'provider_payout_liability', provider: hash(i), rail: 'tap', revision: hash(i),
      target: addr(i), currency: null, chain_id: 1, total_au: amount, held_au: '0', paid_cum_au: '0',
      updated_epoch: 1, holdbacks: [] };
    state.set(liabilityKey(i), row);
    state.set('earn/tap/' + hash(i), { ...row, denom: 'au_usd', pool_address: scope.pool_address });
    state.set('prov/' + hash(i), { provider: hash(i), status: 'active' });
    state.set('payout/binding/tap/' + hash(i) + '/' + hash(i), { type: 'provider_payout_binding',
      provider: hash(i), revision: hash(i), target: addr(i), rail: 'tap', verified: true,
      currency: null, chain_id: 1, network: 'mainnet', payment_config_version: 1, activation_epoch: 1 });
  }
  const head = { number: 5, hash: '0x' + hash(41), timestamp: 10_000 };
  const pool = { getAddress: async () => scope.pool_address, token: async () => scope.token_address,
    epoch: async () => 0n, cumulativeSpent: async () => 0n, pendingRoot: async () => emptyPending(),
    pendingMaxEpochDelta: async () => [0n, 0n, 0n],
    merkleRoot: async () => ZeroHash, merkleRootAtEpoch: async () => ZeroHash,
    maxEpochDelta: async () => 9n, totalDeposited: async () => 1000n,
    governanceNonce: async () => 2n, operatorWithdrawn: async () => 0n, totalBurned: async () => 1n,
    proposeRoot: { staticCall: async () => {} },
    runner: { provider: { send: async method => {
      if (method === 'eth_blockNumber') return '0x5';
      assert.equal(method, 'eth_chainId');
      return '0x1';
    }, getBlock: async () => head, getNetwork: async () => ({ chainId: 1n }) } } };
  let length = 0;
  const snapshots = new Map();
  function seal() { snapshots.set(++length, structuredClone(state)); }
  function epoch(n) {
    const anchor = { type: 'epoch_apply_anchor', epoch: n, apply_hash: hash(n), settlement_unix: n * 100, applied_at: hash(n) };
    state.set('epoch/apply/state', { updated_epoch: n, last_apply_hash: hash(n),
      pending_epoch: null, last_settlement_unix: anchor.settlement_unix, updated_at: anchor.applied_at });
    state.set('epoch/apply-anchor/' + n, anchor);
    seal();
  }
  function oracle(ts, price = rateValue) {
    const record = oracleRecord({ rate_ts: ts, tap_usd_au: String(price), source: 'fixture', posted_by: admin });
    state.set(record.updated_at, record);
    state.set('tap/rate/latest', record);
    seal();
    return record;
  }
  const fetchImpl = async url => {
    const q = url.searchParams;
    assert.equal(q.get('confirmed'), 'true');
    const pin = Number(q.get('signed_length') ?? length);
    const view = snapshots.get(pin);
    assert(view);
    const body = { confirmed: true, signed_length: pin };
    if (q.has('key')) {
      const key = q.get('key');
      return Response.json({ ...body, key, value: view.get(key) ?? null });
    }
    const prefix = q.get('prefix'), limit = Number(q.get('limit'));
    assert.equal(prefix, 'payout/liability/tap/');
    assert(limit <= 32 && limit > 0);
    const keys = [...view.keys()].filter(k => k.startsWith(prefix) && k > (q.get('after') ?? '')).sort();
    return Response.json({ ...body, prefix, truncated: keys.length > limit,
      next_cursor: keys.length > limit ? keys[limit - 1] : null,
      values: keys.slice(0, limit).map(key => ({ key, value: view.get(key) })) });
  };
  const common = { spool, peerRpcUrl: 'file:///fixture/', fetchImpl, pool };
  async function freeze(n, { version = 2, refunds } = {}) {
    const name = 'epoch-' + n + '-' + hash(n), dir = path.join(spool, 'payout', name);
    fs.mkdirSync(dir, { recursive: true });
    const prefix = path.join(spool, 'working', name), file = prefix + '.receipts.json';
    const source = { epoch: n, epoch_apply_hash: hash(n), rail: 'tap', receipts: [],
      ...(refunds && { buyer_refunds: refunds }) };
    write(file, source);
    const args = { epoch: n, anchor: hash(n), spool, dir, peer: common.peerRpcUrl, file, fetchImpl, empty: false };
    await prepareSweep(args);
    if (version === 1) {
      const frozen = path.join(dir, 'tap-sweep-intent.json');
      const legacy = read(frozen);
      delete legacy.refund_queue_version;
      delete legacy.refund_basis;
      write(frozen, { ...legacy, schema_version: 1 });
      write(file, source);
      await prepareSweep(args);
    }
    const bundle = read(file);
    const lock = tapRateRecordToLock(state.get('tap/rate/latest'), bundle, admin, scope);
    write(prefix + '.tap-rate.json', lock);
    write(path.join(dir, 'tap.produced'), { schema_version: 1, epoch: n, epoch_apply_hash: hash(n), status: 'produced',
      rail: 'tap', evidence_sha256: digest(fs.readFileSync(file)) });
    const options = { bundle, epochApplyHash: hash(n), tapRateLock: lock, tapUsdAu: lock.tap_usd_au,
      targetedSessionBindings: {}, canonicalLiabilities: [], settleThroughEpoch: n,
      peerRpcUrl: common.peerRpcUrl, fetchImpl, post: false };
    return { ...common, dir, prefix, file, name, args, bundle, lock, options };
  }
  return { state, seal, epoch, oracle, head, pool, freeze, spool };
}

function unclaimedBasis(f) {
  const providers = [{ account: addr(4), cumulative_wei: '3' }];
  const refunds = [{ account: addr(3), cumulative_wei: '2' }];
  const entries = [...refunds, ...providers];
  const root = distribution(entries.map(row => ({ account: row.account, amount: BigInt(row.cumulative_wei) }))).root;
  const cp = { ...scope, epoch: 3, epoch_apply_hash: hash(3), root, cumulative_spent_wei: '4',
    root_confirmed: true, provider_entries: providers, refunds, entries, buyer_refund_wei: '2', execution_tx: '0x' + hash(73) };
  const prior = { ...cp, providers, tap_rate_lock: scope, operator_fee: { completed: true },
    burn: { completed: true }, tap_settlement_checkpoint: cp };
  write(path.join(f.spool, 'processed/latest.json'), prior);
  write(path.join(f.spool, 'processed', 'epoch-3-' + hash(3) + '.settlement.json'), prior);
  f.state.set('settle/targeted/tap/3', { ...cp, type: 'targeted_tap_settlement', settled_by: admin, settled_by_role: 'admin' });
  f.state.set('settle/targeted/tap/state/1/' + scope.pool_address, { ...scope, type: 'targeted_tap_settlement_state',
    last_epoch: 3, last_root: root, last_execution_tx: cp.execution_tx, cumulative_spent_wei: '4',
    cumulative_provider_claimed_wei: '3', cumulative_buyer_refund_wei: '2' });
  Object.assign(f.pool, { epoch: async () => 3n, cumulativeSpent: async () => 4n, merkleRoot: async () => root,
    claimed: () => assert.fail('unclaimed, unchanged leaves need no claim or readback') });
  f.seal();
  return prior;
}

function finishPage(work, report, status) {
  const prefix = path.join(work.spool, 'processed', work.name);
  fs.renameSync(work.file, prefix + '.receipts.json');
  const reportFile = prefix + '.settlement.json';
  write(reportFile, report);
  let evidence = reportFile;
  if (status === 'settled') {
    evidence = path.join(work.dir, 'reconciliation.json');
    write(evidence, { type: 'canonical_tap_settlement_reconciliation', processed_report_sha256: digest(fs.readFileSync(reportFile)) });
  }
  const marker = path.join(work.dir, 'tap.complete.tmp');
  write(marker, { rail: 'tap', status, epoch: work.bundle.epoch, epoch_apply_hash: work.bundle.epoch_apply_hash,
    evidence, evidence_sha256: digest(fs.readFileSync(evidence)) });
  advanceSweep({ ...work.args, marker });
  advanceSweep({ ...work.args, marker });
}

async function retire(f, work, n, crash) {
  const report = await rollTapSettlement({ ...work.options, pool: f.pool });
  assert(report.blocked && report.reasons.includes('epoch delta > cap'));
  write(work.prefix + '.dry-run.json', report);
  fs.writeFileSync(work.prefix + '.draft-retry', '');
  f.epoch(n);
  const args = { ...work, action: 'retire', successor: { epoch: n, anchor: hash(n) } };
  if (crash) {
    const rename = fs.renameSync;
    const fault = crash.mock.method(fs, 'renameSync', (from, to) => {
      if (to === handoffFile(f.spool)) throw new Error('simulated crash before handoff publication');
      return rename(from, to);
    });
    try { await assert.rejects(recoverDraft(args), /simulated crash/); }
    finally { fault.mock.restore(); }
    assert(!fs.existsSync(work.file) && !fs.existsSync(path.join(work.dir, 'tap.complete')));
    assert.throws(() => validateQueuedSweep(f.spool, work.bundle), /Missing TAP draft handoff/);
  }
  await recoverDraft(args);
  await recoverDraft(args);
  assert(!fs.existsSync(work.file));
  assert.equal(loadCursor(f.spool), null);
}

for (const version of [1, 2]) {
  test('a failed v' + version + ' page isolates one oversized debt and transfers refunds exactly once', async t => {
    const f = fixture(t);
    f.epoch(4); f.oracle(9500);
    const prior = unclaimedBasis(f);
    const first = await f.freeze(4, { version, refunds: [{ account: addr(3), amount_wei: '3' },
      { account: addr(3), amount_au: '3' }, { account: addr(3), amount_au: '3' }] });
    first.options.prior = prior;
    await retire(f, first, 5, t);
    const single = await f.freeze(5);
    assert.equal(single.bundle.tap_liability_sweep.after, null);
    assert.equal(single.bundle.tap_liability_sweep.limit, 1);
    single.options.prior = prior;
    await retire(f, single, 6);
    const payable = await f.freeze(6);
    assert.equal(payable.bundle.tap_liability_sweep.after, liabilityKey(1));
    validateQueuedSweep(f.spool, payable.bundle);
    assert.throws(() => validateQueuedSweep(f.spool, { ...payable.bundle, tap_carried_refunds: [] }), /refunds/);
    const signing = { pool: f.pool, operatorAddress: addr(8),
      ownerSigner: { getAddress: async () => addr(6) }, governanceSigner: {
        getAddress: async () => addr(7), signTypedData: async () => '0x' + '00'.repeat(65) } };
    const report = await rollTapSettlement({ ...payable.options, prior, ...signing });
    assert.equal(report.propose_root_dry_run.static_call_ok, true);
    assert.equal(report.canonical_preparation_confirmed, false);
    assert.deepEqual(report.checkpoint_outputs.map(o => [o.provider, o.net_au_paid, o.tap_wei]), [[hash(2), '12', '6']]);
    assert.deepEqual(report.refunds, [{ account: addr(3), cumulative_wei: '7' }]);
    assert.deepEqual(report.providers, [{ account: addr(2), cumulative_wei: '6' }, ...prior.providers]);
    assert.equal(report.root, distribution([{ account: addr(2), amount: 6n }, { account: addr(3), amount: 7n }, { account: addr(4), amount: 3n }]).root);
    assert.equal(report.cumulative_spent_wei, '12');
    assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');
    assert((await guardianPreSignReport({ settlement: report, epoch: 6, previous: prior, prevSpentWei: 4n,
      totalDepositedWei: 1000n, maxEpochDeltaWei: 9n })).ok);
    const processed = path.join(f.spool, 'processed', payable.name);
    fs.renameSync(payable.file, processed + '.receipts.json');
    const evidence = path.join(payable.dir, 'reconciliation.json');
    const marker = path.join(payable.dir, 'tap.complete.tmp');
    const plan = await buildCanonicalTapPreparationPlan({ settlement: report, bundle: payable.bundle,
      epoch: 6, epochApplyHash: hash(6), tapRateLock: payable.lock });
    const cp = { ...plan.intent, root_confirmed: true, execution_tx: '0x' + hash(76) };
    const settled = { ...report, root_confirmed: true, tap_settlement_checkpoint: cp,
      operator_fee: { completed: true }, burn: { completed: true } };
    const commit = refunds => {
      write(processed + '.settlement.json', { ...settled, refunds, tap_settlement_checkpoint: { ...cp, refunds } });
      write(evidence, { type: 'canonical_tap_settlement_reconciliation',
        processed_report_sha256: digest(fs.readFileSync(processed + '.settlement.json')) });
      write(marker, { rail: 'tap', status: 'settled', epoch: 6, epoch_apply_hash: hash(6), evidence,
        evidence_sha256: digest(fs.readFileSync(evidence)) });
      advanceSweep({ ...payable.args, marker });
    };
    assert.throws(() => commit([]), /refund/);
    assert.equal(loadCursor(f.spool), null);
    commit(report.refunds); commit(report.refunds);
    assert.deepEqual(loadCursor(f.spool).refund_queue, { head: 1, offset: 0, tail: 1 });
    const core = await confirmedTapContract(payable.peerRpcUrl, payable.fetchImpl);
    const updates = await core.targetedPayoutSettlementUpdates(plan.intent.outputs.map(o => ({ ...o, au: o.paid_au,
      role: 'provider' })), 'tap', 6, 600, plan.preparation_ids);
    for (const { key, value } of updates.liabilityUpdates) f.state.set(key, value);
    for (const row of updates.earningUpdates) f.state.set(core.earningKey(row.provider, 'tap'), row);
    assert.equal(f.state.get(liabilityKey(2)).paid_cum_au, '12');
    assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');
    f.seal();
    await assert.rejects(preflightTapSweepPreparations(plan, payable.peerRpcUrl, payable.fetchImpl, prior), /watermark/);
    write(path.join(f.spool, 'processed/latest.json'), settled);
    f.state.set('settle/targeted/tap/6', { ...cp, type: 'targeted_tap_settlement', settled_by: admin, settled_by_role: 'admin' });
    f.state.set('settle/targeted/tap/state/1/' + scope.pool_address, { ...scope, type: 'targeted_tap_settlement_state',
      last_epoch: 6, last_root: cp.root, last_execution_tx: cp.execution_tx, cumulative_spent_wei: '12',
      cumulative_provider_claimed_wei: '9', cumulative_buyer_refund_wei: '7' });
    Object.assign(f.pool, { epoch: async () => 6n, cumulativeSpent: async () => 12n,
      merkleRoot: async () => report.root, maxEpochDelta: async () => 100n });
    f.epoch(7);
    const next = await f.freeze(7);
    assert.equal(next.bundle.tap_carried_refunds, undefined);
    assert.equal(next.bundle.tap_liability_sweep.after, null); // unpaid first row is revisited
    const retried = await rollTapSettlement({ ...next.options, prior: settled, ...signing });
    assert.equal(retried.propose_root_dry_run.static_call_ok, true);
    assert.deepEqual(retried.checkpoint_outputs.map(o => [o.provider, o.net_au_paid, o.tap_wei]), [[hash(1), '101', '50']]);
    assert.deepEqual(retried.refunds, report.refunds);
    assert.equal(retried.cumulative_spent_wei, '79');
  });
}

test('a stale local quote changes only before any preparation, pending root or ambiguous send', async t => {
  const f = fixture(t);
  f.epoch(4); f.oracle(1000);
  const work = await f.freeze(4);
  f.oracle(9900, rateValue * 2n);
  const old = fs.readFileSync(work.prefix + '.tap-rate.json');
  const reprice = () => recoverDraft({ ...work, action: 'reprice' });
  for (const ext of ['.prepare-intent.json', '.settlement.raw.json', '.settlement.json']) {
    fs.writeFileSync(work.prefix + ext, '');
    await assert.rejects(reprice(), /previously started/);
    fs.unlinkSync(work.prefix + ext);
  }
  const lockKey = 'payout/preparation-lock/tap/' + hash(2) + '/' + hash(2) + '/0';
  f.state.set(lockKey, { economic_op_id: hash(42), rail: 'tap', provider: hash(2),
    payout_revision: hash(2), paid_cum_au_before: '0' });
  const prepKey = 'payout/preparation/tap/' + hash(42);
  f.state.set(prepKey, { type: 'targeted_payout_preparation', economic_op_id: hash(42), rail: 'tap',
    epoch: 4, epoch_apply_hash: hash(4), kind: 'liability', consumed: false, liability: {
      provider: hash(2), payout_revision: hash(2), paid_cum_au_before: '0' } });
  f.seal();
  await assert.rejects(reprice(), /prior preparation/);
  f.state.delete(lockKey); f.state.delete(prepKey); f.seal();
  const report = await rollTapSettlement(work.options);
  const plan = await buildCanonicalTapPreparationPlan({ settlement: report, bundle: work.bundle,
    epoch: 4, epochApplyHash: hash(4), tapRateLock: work.lock });
  journalTapPreparation(work.prefix + '.draft-plan.json', plan, true);
  for (const key of ['payout/preparation/tap/' + plan.root_preparation_id,
    'payout/preparation-effect/tap/' + plan.external_effect_ids[0]]) {
    f.state.set(key, { consumed: false }); f.seal();
    await assert.rejects(reprice(), /TAP (preparation|effect) exists/);
    f.state.delete(key); f.seal();
  }
  f.state.get(liabilityKey(2)).paid_cum_au = '6'; f.seal();
  await assert.rejects(reprice(), /watermark advanced/);
  f.state.get(liabilityKey(2)).paid_cum_au = '0'; f.seal();
  for (const pending of [[], [ZeroHash, 4n, 0n, 0n, 0n, 0n, 0n], ['0x' + hash(41), 4n, 8n, 0n, 0n, 0n, 12000n]]) {
    f.pool.pendingRoot = async () => pending;
    await assert.rejects(reprice(), /ambiguous root/);
  }
  f.pool.pendingRoot = async () => emptyPending();
  f.pool.pendingMaxEpochDelta = async () => [12n, 2n, 12000n];
  await assert.rejects(reprice(), /pending governance/);
  f.pool.pendingMaxEpochDelta = async () => [0n, 0n, 0n];
  f.pool.merkleRootAtEpoch = async () => '0x' + hash(41);
  await assert.rejects(reprice(), /ambiguous root/);
  f.pool.merkleRootAtEpoch = async () => ZeroHash;
  assert.deepEqual(fs.readFileSync(work.prefix + '.tap-rate.json'), old);
  assert.equal(await reprice(), true);
  const lock = read(work.prefix + '.tap-rate.json');
  assert.equal(lock.tap_usd_au, String(rateValue * 2n));
  assert.equal(lock.bundle_sha256, work.lock.bundle_sha256);
  assert.equal(await reprice(), false);
  const repriced = await rollTapSettlement({ ...work.options, tapRateLock: lock, tapUsdAu: lock.tap_usd_au });
  assert.deepEqual(repriced.checkpoint_outputs.map(o => o.tap_wei), ['25', '3']);
  assert.equal(f.state.get(liabilityKey(2)).paid_cum_au, '0');
  assert(!fs.existsSync(handoffFile(f.spool)));
});

test('an empty page retains buyer instructions and a paid page rotates only the still unpaid refunds', async t => {
  const f = fixture(t);
  function accrue(i, amount) {
    for (const key of [liabilityKey(i), 'earn/tap/' + hash(i)]) f.state.get(key).total_au = amount;
  }
  accrue(1, '0'); accrue(2, '0');
  f.epoch(4); f.oracle(9500);
  const buyer = hash(77);
  const unresolved = [{ buyer, amount_wei: '3' }, { account: addr(9), amount_au: '1' }];
  const own = [...unresolved, { account: addr(8), amount_wei: '2' }];
  const first = await f.freeze(4, { refunds: own });
  const carry = await rollTapSettlement({ ...first.options, pool: f.pool });
  assert.deepEqual(carry.checkpoint_outputs, []);
  assert.deepEqual(carry.refunds, []);
  assert.equal(carry.root, undefined);
  finishPage(first, carry, 'carry');
  assert.deepEqual(loadCursor(f.spool).refund_queue, { head: 0, offset: 0, tail: 1 });
  accrue(2, '12'); f.epoch(5);
  const second = await f.freeze(5);
  assert.deepEqual(second.bundle.tap_carried_refunds, own);
  const report = await rollTapSettlement(second.options);
  assert.equal(report.checkpoint_outputs[0].tap_wei, '6');
  assert.deepEqual(report.refunds, [{ account: addr(8), cumulative_wei: '2' }]);
  assert.equal(report.pending_refund_wei, '3');
  assert(!(await guardianPreSignReport({ settlement: report, epoch: 5, totalDepositedWei: 12n })).ok);
  assert((await guardianPreSignReport({ settlement: report, epoch: 5, totalDepositedWei: 13n })).ok);
  finishPage(second, { ...report, tap_settlement_checkpoint: { refunds: report.refunds } }, 'settled');
  assert.deepEqual(loadCursor(f.spool).refund_queue, { head: 1, offset: 0, tail: 2 });
  f.epoch(6); f.oracle(9900, rateValue / 2n);
  const last = await f.freeze(6);
  assert.deepEqual(last.bundle.tap_carried_refunds, unresolved);
  const recovered = projectTapRefunds(last.bundle, { [buyer]: addr(3) }, last.lock.tap_usd_au, true);
  assert.deepEqual(recovered.credits, [{ account: addr(3), amount_wei: '3' }, { account: addr(9), amount_wei: '1' }]);
  assert.deepEqual(recovered.unpaid, []);
});

test('a processed historical v1 carry retains an AU refund worth zero wei at its original rate', async t => {
  const f = fixture(t);
  for (const i of [1, 2]) {
    f.state.get(liabilityKey(i)).total_au = '0';
    f.state.get('earn/tap/' + hash(i)).total_au = '0';
  }
  f.epoch(4); f.oracle(9500);
  const refunds = [{ account: addr(3), amount_au: '1' }];
  const old = await f.freeze(4, { version: 1, refunds });
  assert.equal(old.bundle.tap_refund_queue_version, undefined);
  const carry = await rollTapSettlement({ ...old.options, pool: f.pool });
  assert.deepEqual(carry.refunds, []);
  finishPage(old, carry, 'carry');
  f.epoch(5);
  assert.deepEqual((await f.freeze(5)).bundle.tap_carried_refunds, refunds);
});

test('the direct CLI and shell service share the same exclusive spool lock', t => {
  const f = fixture(t);
  const script = `import fcntl, os, subprocess, sys
lock, *node = sys.argv[1:]
with open(lock, 'a') as handle:
    os.dup2(handle.fileno(), 8)
    fcntl.flock(8, fcntl.LOCK_EX)
    assert subprocess.run(node, pass_fds=(8,)).returncode == 0
    assert subprocess.run(node, stderr=subprocess.DEVNULL).returncode != 0
    os.close(8)
assert subprocess.run(node).returncode == 0`;
  const file = path.join(f.spool, 'roller.lock');
  const helper = new URL('../scripts/tap-draft-files.mjs', import.meta.url).href;
  const result = spawnSync('python3', ['-c', script, file, process.execPath, '--input-type=module', '-e',
    'import {holdTapLock} from ' + JSON.stringify(helper) + '; holdTapLock(process.argv[1], 8)', file],
    { encoding: 'utf8', timeout: 5000 });
  assert.equal(result.status, 0, result.stderr);
});

test('the payout worker finishes an interrupted handoff and releases only that queue entry', async t => {
  const f = fixture(t);
  f.epoch(4); f.oracle(9500);
  const work = await f.freeze(4);
  fs.writeFileSync(work.prefix + '.draft-retry', '');
  f.epoch(5);
  const link = fs.linkSync;
  const fault = t.mock.method(fs, 'linkSync', (from, to) => {
    if (to === path.join(work.dir, 'tap.complete')) throw Error('simulated marker crash');
    return link(from, to);
  });
  try { await assert.rejects(recoverDraft({ ...work, action: 'retire', successor: { epoch: 5, anchor: hash(5) } }), /marker crash/); }
  finally { fault.mock.restore(); }
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const worker = fs.readFileSync(path.join(repo, 'scripts/ops-payout-settle.sh'), 'utf8');
  const extract = (start, next) => worker.slice(worker.indexOf(start + '() {'), worker.indexOf(next + '() {'));
  const head = path.join(f.spool, 'payout', 'epoch-5-' + hash(5));
  fs.mkdirSync(head);
  const queue = () => spawnSync('python3', [path.join(repo, 'scripts/ops/payout-work-queue.py'),
    'has-prior', head, 'tap'], { encoding: 'utf8' });
  assert.equal(queue().stdout.trim(), 'yes');
  const result = spawnSync('bash', ['-eu', '-c',
    extract('validate_rail_marker', 'mark_tap_produced') + extract('produce_tap_work', 'supersede_unplanned_prior_rail_work') +
    '\nflock() { python3 -c \'import fcntl,sys; fcntl.flock(int(sys.argv[1]), fcntl.LOCK_EX | fcntl.LOCK_NB)\' "$2"; }\nproduce_tap_work'],
    { encoding: 'utf8', timeout: 5000, env: { PATH: process.env.PATH, work_dir: work.dir,
      SOURCE_DIR: repo, TAP_SPOOL: f.spool, TAP_SWEEP_ENABLED: '1', applied_epoch: '4',
      apply_hash: hash(4), canonical_applied_epoch: '5', canonical_apply_hash: hash(5),
      canonical_settlement_unix: '400', RPC_URL: 'file:///fixture/' } });
  assert.equal(result.status, 0, result.stderr);
  const sync = spawnSync('python3', [path.join(repo, 'scripts/ops/payout-work-queue.py'), 'sync', work.dir]);
  assert.equal(sync.status, 0);
  assert.equal(queue().stdout.trim(), 'no');
  assert.equal(read(path.join(work.dir, 'tap.complete')).status, 'superseded');
  assert.equal(loadCursor(f.spool), null);
  assert.equal((await f.freeze(5)).bundle.tap_liability_sweep.limit, 1);
});

test('a queued historical epoch uses its fully applied signed checkout', async t => {
  const f = fixture(t);
  f.epoch(4); f.oracle(9500);
  f.epoch(5);
  const work = await f.freeze(4);
  assert.equal(work.bundle.tap_liability_sweep.signed_length, 2);
  assert.equal(work.bundle.at, 400);
  assert.equal(work.bundle.epoch_apply_hash, hash(4));
});

test('a historical failed bundle keeps every refund in bounded slices and honors working submission evidence', async t => {
  const f = fixture(t);
  f.epoch(4); f.oracle(9500);
  const refunds = Array.from({ length: 35 }, (_, i) => ({ account: addr(8), amount_wei: String(i + 1) }));
  const work = await f.freeze(4, { version: 1, refunds });
  const failed = path.join(f.spool, 'failed', work.name + '.receipts.json');
  fs.renameSync(work.file, failed);
  const incomplete = read(failed);
  delete incomplete.params.fee_bps;
  write(failed, incomplete);
  const marker = path.join(work.dir, 'tap.produced');
  write(marker, { ...read(marker), evidence_sha256: digest(fs.readFileSync(failed)) });
  f.epoch(5);
  const recover = () => recoverDraft({ ...work, action: 'retire', successor: { epoch: 5, anchor: hash(5) } });
  const historical = work.prefix + '.settlement.raw.json';
  fs.writeFileSync(historical, '');
  await assert.rejects(recover(), /previously started/);
  assert(fs.existsSync(failed));
  fs.unlinkSync(historical);
  await recover();
  const first = nextRefunds(f.spool, { head: 0, offset: 0, tail: 1 });
  const rest = nextRefunds(f.spool, first.next);
  assert.equal(first.entries.length, 32);
  assert.deepEqual([...first.entries, ...rest.entries], refunds);
  assert.equal(nextRefunds(f.spool, rest.next), null);
  assert.equal(loadCursor(f.spool), null);
});

test('reservation rehearsal rejects a changed second liability before reserving the first', async t => {
  const f = fixture(t);
  f.epoch(4); f.oracle(9500);
  const work = await f.freeze(4);
  const report = await rollTapSettlement(work.options);
  const plan = await buildCanonicalTapPreparationPlan({ settlement: report, bundle: work.bundle, epoch: 4,
    epochApplyHash: hash(4), tapRateLock: work.lock });
  await preflightTapSweepPreparations(plan, work.peerRpcUrl, work.fetchImpl);
  f.state.get(liabilityKey(2)).paid_cum_au = '12';
  f.seal();
  await assert.rejects(preflightTapSweepPreparations(plan, work.peerRpcUrl, work.fetchImpl), /watermark/);
  assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');
  assert(![...f.state.keys()].some(k => k.startsWith('payout/preparation')));
});
