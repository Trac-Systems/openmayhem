import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { ethers, ZeroHash } from 'ethers';
import MayhemContract from '../../intercom/contract/contract.js';
import { executeFeature, executeRateFeature, makeIdentity, makeVerifier, MemoryStorage } from '../../intercom/tests/helpers/contract.js';
import { advanceSweep, loadCursor, prepareSweep, validateQueuedSweep, verifyQueuedTapSweep } from '../scripts/prepare-tap-sweep-bundle.mjs';
import { digest, handoffFile, journalTapPreparation, sealTapSource, nextRefunds } from '../scripts/tap-draft-files.mjs';
import { proveUnpreparedTapDraft, recoverDraft } from '../scripts/tap-draft-recovery.mjs';
import { runDebtTimer } from './helpers/tap-debt-timer.mjs';
import { dispatchFencedTapTick } from '../scripts/tap-debt-worker.mjs';
import { confirmedTapContract, preflightTapSweepPreparations, verifyPriorTapClaims } from '../scripts/tap-liability-sweep.mjs';
import { buildCanonicalTapPreparationPlan, guardianPreSignReport, projectTapRefunds, rollTapSettlement,
  resolveTapSettlementPayoutMinimum, resolveTargetedTapPayoutsFromLedger, submitCanonicalTapPreparationPlan, tapRateRecordToLock } from '../scripts/tap-settlement-roller.mjs';
import { distribution } from '../scripts/merkle.mjs';
import { oracleRecord } from './helpers/tap-pricing.mjs';
import { checkTapProcessedReport, checkTapSpoolReport } from './helpers/tap-spool-report.mjs';
import { verifyTapSweepReport } from '../scripts/verify-tap-sweep-report.mjs';

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

function fixture(t, signer = null) {
  const admin = signer?.publicKey ?? hash(91);
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
  const storage = new MemoryStorage();
  storage.values = state;
  const reducer = signer && new MayhemContract({ peer: { wallet: makeVerifier(signer.wallet) } }, {});
  const fetchImpl = async (url, options) => {
    if (options?.method === 'POST') {
      const { key, value } = JSON.parse(options.body);
      const result = await executeFeature(reducer, storage, 'mayhem_feature', key, value, admin);
      assert.equal(result.ok, true, result.message);
      seal();
      return Response.json(result);
    }
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
  return { ...common, state, seal, epoch, oracle, head, freeze, reducer, storage };
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
    const cp = { ...plan.intent, root_preparation_id: plan.root_preparation_id, root_confirmed: true,
      proposal_tx: '0x' + hash(75), execution_tx: '0x' + hash(76) };
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
    f.state.set('payout/preparation/tap/' + cp.root_preparation_id, { ...plan.preparations.at(-1),
      type: 'targeted_payout_preparation', rail: 'tap', epoch: 6, epoch_apply_hash: hash(6),
      consumed: true, consumed_by: 'settle/targeted/tap/6' });
    f.state.set('rail/seen/tap-settlement-root/1/' + scope.pool_address + '/' + cp.root, {
      rail: 'tap', purpose: 'targeted_settlement', epoch: 6, epoch_apply_hash: hash(6), root: cp.root,
      proposal_tx: cp.proposal_tx, execution_tx: cp.execution_tx });
    f.state.set('settle/targeted/tap/state/1/' + scope.pool_address, { ...scope, type: 'targeted_tap_settlement_state',
      last_epoch: 6, last_root: cp.root, last_execution_tx: cp.execution_tx, cumulative_spent_wei: '12',
      cumulative_provider_claimed_wei: '9', cumulative_buyer_refund_wei: '7' });
    Object.assign(f.pool, { epoch: async () => 6n, cumulativeSpent: async () => 12n,
      merkleRoot: async () => report.root, maxEpochDelta: async () => 100n });
    f.epoch(7);
    const swapped = read(path.join(f.spool, 'processed/latest.json'));
    swapped.providers = [settled.providers[0], { account: addr(3), cumulative_wei: '1' }, { account: addr(4), cumulative_wei: '2' }];
    swapped.refunds = [{ account: addr(3), cumulative_wei: '6' }, { account: addr(4), cumulative_wei: '1' }];
    Object.assign(swapped.tap_settlement_checkpoint, { provider_entries: swapped.providers, refunds: swapped.refunds });
    await assert.rejects(verifyPriorTapClaims(await confirmedTapContract(payable.peerRpcUrl, payable.fetchImpl), swapped),
      /claim distribution/); // identical account totals and root, different owners of the refund
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
  f.epoch(3); f.epoch(4); f.oracle(1000);
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
  const earlier = f.state.get(prepKey);
  Object.assign(earlier, { epoch: 3, epoch_apply_hash: hash(3) });
  Object.assign(earlier.liability, { paid_au: '6', aggregate_paid_cum_au_before: '0' });
  const tailKey = 'payout/preparation-tail/tap/' + hash(2);
  f.state.set(tailKey, { economic_op_id: hash(42), consumed: false, paid_cum_au_before: '0', paid_cum_au_after: '6' });
  f.seal();
  await proveUnpreparedTapDraft({ ...work, prior: null, reports: [] });
  assert.equal(f.state.get(prepKey).consumed, false);
  f.state.delete(tailKey); f.state.delete(lockKey); f.state.delete(prepKey); f.seal();
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

function resumePayoutWorker(f, work, nextEpoch) {
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const worker = fs.readFileSync(path.join(repo, 'scripts/ops-payout-settle.sh'), 'utf8');
  const extract = (start, next) => worker.slice(worker.indexOf(start + '() {'), worker.indexOf(next + '() {'));
  const head = path.join(f.spool, 'payout', 'epoch-' + nextEpoch + '-' + hash(nextEpoch));
  fs.mkdirSync(head, { recursive: true });
  const queue = () => spawnSync('python3', [path.join(repo, 'scripts/ops/payout-work-queue.py'),
    'has-prior', head, 'tap'], { encoding: 'utf8' });
  assert.equal(queue().stdout.trim(), 'yes');
  const result = spawnSync('bash', ['-eu', '-c',
    extract('validate_rail_marker', 'mark_tap_produced') + extract('produce_tap_work', 'supersede_unplanned_prior_rail_work') +
    '\nflock() { python3 -c \'import fcntl,sys; fcntl.flock(int(sys.argv[1]), fcntl.LOCK_EX | fcntl.LOCK_NB)\' "$2"; }\nproduce_tap_work'],
    { encoding: 'utf8', timeout: 5000, env: { PATH: process.env.PATH, work_dir: work.dir,
      SOURCE_DIR: repo, TAP_SPOOL: f.spool, TAP_SWEEP_ENABLED: '1', applied_epoch: String(work.bundle.epoch),
      apply_hash: work.bundle.epoch_apply_hash, canonical_applied_epoch: String(nextEpoch), canonical_apply_hash: hash(nextEpoch),
      canonical_settlement_unix: String(work.bundle.at), RPC_URL: 'file:///fixture/' } });
  assert.equal(result.status, 0, result.stderr);
  const sync = spawnSync('python3', [path.join(repo, 'scripts/ops/payout-work-queue.py'), 'sync', work.dir]);
  assert.equal(sync.status, 0);
  assert.equal(queue().stdout.trim(), 'no');
}

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
  resumePayoutWorker(f, work, 5);
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
  // Recovery streams historical receipt data without charging it against
  // the bounded financial header or treating an unread receipt as a payment.
  incomplete.receipts = [{ retained: 'x'.repeat(17 * 1024 * 1024) }];
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

// Install the constructor's bytecode and storage at the two addresses that the
// production selector authenticates. All RPC calls go to an in-process Ganache.
async function canonicalPoolFixture(t) {
  const { default: Ganache } = await import('ganache');
  const { compileAll } = await import('../scripts/compile.mjs');
  const { deployPoolWithToken } = await import('../scripts/deploy-local.mjs');
  const chain = Ganache.provider({ logging: { quiet: true }, chain: { chainId: 1, time: new Date(10000000) }, miner: { timestampIncrement: 0 } });
  t.after(() => chain.disconnect());
  const provider = new ethers.BrowserProvider(chain, undefined, { cacheTimeout: -1 });
  const owner = await provider.getSigner();
  const governance = ethers.Wallet.createRandom();
  const art = compileAll();
  const tokenCode = await provider.call({ data: art.MockTTAP.bytecode });
  await provider.send('evm_setAccountCode', [scope.token_address, tokenCode]);
  const template = await deployPoolWithToken(owner, scope.token_address, {
    governanceSigner: governance.address, governanceDelay: 3600n, maxEpochDelta: 100n, art,
  });
  await provider.send('evm_setAccountCode', [scope.pool_address, await provider.getCode(template.poolAddr)]);
  const trace = await provider.send('debug_traceTransaction', [template.pool.deploymentTransaction().hash, {}]);
  const slots = new Set(trace.structLogs.filter(step => step.op === 'SSTORE').map(step => '0x' + step.stack.at(-1)));
  for (const slot of slots) {
    const word = await provider.getStorage(template.poolAddr, slot);
    await provider.send('evm_setAccountStorageAt', [scope.pool_address, slot, word]);
  }
  const token = new ethers.Contract(scope.token_address, art.MockTTAP.abi, owner);
  const pool = new ethers.Contract(scope.pool_address, art.MayhemInferencePool.abi, owner);
  assert.equal(await pool.owner(), owner.address);
  assert.equal(await pool.maxEpochDelta(), 100n);
  await (await token.mint(owner.address, 1000n)).wait();
  await (await token.approve(scope.pool_address, 1000n)).wait();
  await (await pool.deposit(1000n)).wait();
  const jump = async seconds => {
    await provider.send('evm_increaseTime', [seconds]);
    await provider.send('evm_mine', []);
  };
  return { pool, provider, token, jump, ownerSigner: owner, governanceSigner: governance, operatorAddress: addr(8) };
}

test('a confirmed reservation is fenced after expiry and restart while a newer provider is actually paid once', async t => {
  const signer = await makeIdentity();
  const f = fixture(t, signer);
  const eth = await canonicalPoolFixture(t);
  const { pool, provider } = eth;
  const oracle = async price => {
    const ts = (await provider.getBlock('latest')).timestamp;
    const result = await executeRateFeature(f.reducer, f.storage, {
      op: 'tap_rate_oracle', source: 'uniswap-v3-twap', ts, tap_usd_au: String(price),
    }, signer.publicKey);
    assert.equal(result.ok, true, result.message);
    f.seal();
  };
  const later = f.state.get(liabilityKey(2));
  f.state.delete(liabilityKey(2));
  f.epoch(3);
  await oracle(rateValue / 2n);
  const inherited = { account: addr(9), amount_au: '8' };
  const earlier = await f.freeze(3, { refunds: [inherited] });
  const oversized = await rollTapSettlement({ ...earlier.options, ...eth, post: false });
  assert(oversized.reasons.includes('epoch delta > cap'));
  write(earlier.prefix + '.dry-run.json', oversized);
  fs.writeFileSync(earlier.prefix + '.draft-retry', '');
  f.epoch(4);
  await recoverDraft({ ...earlier, pool, action: 'retire', successor: { epoch: 4, anchor: hash(4) } });
  await eth.jump(1);
  await oracle(rateValue);
  const work = await f.freeze(4, { refunds: [{ account: addr(9), amount_au: '4' }] });
  const adminSigner = { publicKey: signer.publicKey,
    sign: message => Buffer.from(signer.wallet.sign(Buffer.from(message))).toString('hex') };
  assert.deepEqual(work.bundle.tap_carried_refunds, [inherited]);
  const run = (item, options) => {
    const bundle = read(item.file);
    const tapRateLock = read(item.prefix + '.tap-rate.json');
    const latest = path.join(item.spool, 'processed/latest.json');
    return rollTapSettlement({ ...item.options, ...eth, bundle, tapRateLock, tapUsdAu: tapRateLock.tap_usd_au,
      prior: fs.existsSync(latest) ? read(latest) : null, post: true,
      checkQueuedSweep: () => verifyQueuedTapSweep(f.spool, bundle, pool),
      observeDraftPlan: plan => journalTapPreparation(item.prefix + '.draft-plan.json', plan, true),
      canonicalPreparationSubmitter: request => submitCanonicalTapPreparationPlan({ ...request, ...item, adminSigner,
        beforePost: () => journalTapPreparation(item.prefix + '.prepare-intent.json', request.plan) }),
      ...options });
  };
  // Crash between confirmation of all reservations and the EVM invocation.
  await assert.rejects(run(work, { beforeEthereumEffect: () => { throw Error('crash before proposal'); } }),
    /crash before proposal/);
  const journal = read(work.prefix + '.prepare-intent.json');
  const rows = () => [...f.state].filter(([key]) => key.startsWith('payout/preparation') &&
    (key.includes(journal.root_preparation_id) || journal.preparation_ids.some(id => key.includes(id)) ||
      journal.external_effect_ids.some(id => key.includes(id)) || key.includes('/' + hash(1))));
  const reserved = structuredClone(rows());
  assert.equal(f.state.get('payout/preparation/tap/' + journal.root_preparation_id).consumed, false);
  const successor = { epoch: 5, anchor: hash(5) };
  f.state.set(liabilityKey(2), later);
  f.epoch(5);
  fs.writeFileSync(work.prefix + '.draft-retry', '');
  const recover = action => recoverDraft({ ...work, pool, action, successor });
  await assert.rejects(recover('fence'), /authentic and expired/);
  await eth.jump(2701);
  await oracle(rateValue * 2n);
  await assert.rejects(run(work), /oracle is stale/);
  await assert.rejects(recover('reprice'), /previously started/);
  await assert.rejects(recover('retire'), /previously started/);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 0);
  assert(fs.existsSync(work.file));
  // A process opened the legacy confirmation file and lost its reply.
  fs.writeFileSync(work.prefix + '.settlement.raw.json', '');
  const rename = fs.renameSync;
  const crash = t.mock.method(fs, 'renameSync', (from, to) => {
    if (from === work.file) throw Error('crash after committed fence');
    return rename(from, to);
  });
  try { await assert.rejects(recover('fence'), /crash after committed fence/); }
  finally { crash.mock.restore(); }
  assert(fs.existsSync(work.file));
  assert.throws(() => validateQueuedSweep(f.spool, work.bundle), /ENOENT/);
  await assert.rejects(recover('reprice'), /already fenced/);
  resumePayoutWorker(f, work, 5);
  const archive = path.join(work.spool, 'deferred', work.name);
  assert.equal(read(path.join(work.dir, 'tap.complete')).status, 'fenced');
  assert.deepEqual(read(archive + '.prepare-intent.json'), journal);
  assert.deepEqual(read(archive + '.tap-rate.json'), work.lock);
  assert.deepEqual(read(archive + '.draft.json').observed_liability_preparations, journal.preparation_ids);
  assert(fs.existsSync(archive + '.settlement.raw.json'));
  assert.deepEqual(rows(), reserved);
  assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');

  const fresh = await f.freeze(5);
  assert.equal(fresh.bundle.tap_carried_refunds, undefined);
  const pending = await run(fresh);
  assert.equal(pending.awaiting_governance_delay, true);
  assert.deepEqual(pending.tap_liability_sweep.deferred.map(row => row.economic_op_id), journal.preparation_ids);
  assert.deepEqual(pending.checkpoint_outputs.map(row => row.provider), [hash(2)]);
  assert.deepEqual(pending.refunds, []);
  assert.equal((await run(fresh)).proposal_tx, null);
  await eth.jump(3601);
  const executing = await run(fresh);
  assert.equal(executing.awaiting_finality, true);
  for (let i = 0; i < 12; i++) await provider.send('evm_mine', []);
  const final = await run(fresh, { prior: executing });
  const cp = final.tap_settlement_checkpoint;
  final.payout_minimum_lock = (await resolveTapSettlementPayoutMinimum({ ...fresh, at: fresh.bundle.at,
    signedLength: fresh.bundle.tap_liability_sweep.signed_length })).evidence;
  await verifyTapSweepReport({ ...fresh, report: final, rateLock: fresh.lock, confirmed: true,
    proposalBlock: await provider.getBlock(cp.proposal_block_number) });
  const bound = checkTapSpoolReport(fresh.bundle, final);
  assert.equal(bound.finalizable, true, bound.error || bound.gateError);
  const processed = checkTapProcessedReport(fresh.bundle, bound.bound);
  assert.equal(processed.status, 0, processed.error);
  assert.equal(cp.outputs[0].paid_au, '12');
  assert.equal(cp.outputs[0].tap_wei, '3');
  const checkpoint = () => executeFeature(f.reducer, f.storage, 'mayhem_feature',
    key, cp, signer.publicKey);
  const key = await f.reducer.targetedTapSettlementFeatureKey(cp);
  assert.equal((await checkpoint()).idempotent, false);
  assert.equal((await checkpoint()).idempotent, true);
  f.seal();
  assert.equal(f.state.get(liabilityKey(2)).paid_cum_au, '12');
  assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');
  assert.deepEqual(rows(), reserved);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 1);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted())).length, 1);
  // A single leaf has an empty proof. Exercise the normal token payment too.
  await (await pool.claim(5, later.target, 3n, [])).wait();
  assert.equal(await eth.token.balanceOf(later.target), 3n);
  await assert.rejects(pool.claim.staticCall(5, later.target, 3n, []));
  write(path.join(f.spool, 'processed/latest.json'), final);
  finishPage(fresh, final, 'settled');
  for (const key of [liabilityKey(2), 'earn/tap/' + hash(2)]) {
    f.state.set(key, { ...f.state.get(key), total_au: '24', updated_epoch: 6 });
  }
  f.epoch(6);
  await oracle(rateValue / 2n);
  const after = await f.freeze(6);
  assert.deepEqual(after.bundle.tap_carried_refunds, [{ account: addr(9), amount_au: '4' }, inherited]);
  await verifyQueuedTapSweep(f.spool, after.bundle, pool);
  await assert.rejects(verifyQueuedTapSweep(f.spool, after.bundle, {
    runner: pool.runner, epoch: pool.epoch, merkleRootAtEpoch: async () => cp.root,
  }), /irrevocably skipped/);
  const refundProposal = await run(after);
  assert.equal(refundProposal.awaiting_governance_delay, true);
  assert.deepEqual(refundProposal.refunds, [{ account: addr(9), cumulative_wei: '12' }]);
  await eth.jump(3601);
  const refundExecution = await run(after);
  for (let i = 0; i < 12; i++) await provider.send('evm_mine', []);
  const refundReport = await run(after, { prior: refundExecution });
  const refundCheckpoint = refundReport.tap_settlement_checkpoint;
  const settlement = await executeFeature(f.reducer, f.storage, 'mayhem_feature',
    await f.reducer.targetedTapSettlementFeatureKey(refundCheckpoint), refundCheckpoint, signer.publicKey);
  assert.equal(settlement.ok, true, settlement.message);
  const tree = distribution(refundReport.entries.map(row => ({ account: row.account, amount: BigInt(row.cumulative_wei) })));
  await (await pool.claim(6, addr(9), 12n, tree.proofFor(addr(9)))).wait();
  assert.equal(await eth.token.balanceOf(addr(9)), 12n);
  finishPage(after, refundReport, 'settled');
  assert.equal(nextRefunds(f.spool, loadCursor(f.spool).refund_queue), null);
  assert.deepEqual(rows(), reserved);
});

test('a v30 pre-journal mixed receipt job with a lost Core acknowledgement requires positive reservation and idle pool evidence', async t => {
  const { proxyPayoutEpoch, matureFixtureLiabilities } = await import('../../intercom/tests/helpers/proxy-payout-epoch.mjs');
  const { deriveTapReceiptBundle } = await import('../../intercom/scripts/payout-tap-receipts.mjs');
  const source = await proxyPayoutEpoch('tap', { mixed: true });
  await matureFixtureLiabilities(source);
  const signer = source.f.admin;
  const f = fixture(t, signer);
  for (const [key, value] of source.f.storage.values) {
    if (key !== 'payments/current') f.state.set(key, value);
  }
  const bundle = await deriveTapReceiptBundle(source.bundle, source.epoch, source.applyHash);
  assert.equal(bundle.receipts.length, 2);
  const at = f.state.get('epoch/apply-anchor/' + source.epoch).settlement_unix;
  f.head.timestamp = at;
  const lock = tapRateRecordToLock(f.oracle(at), bundle, signer.publicKey, scope);
  const name = 'epoch-' + source.epoch + '-' + source.applyHash;
  const dir = path.join(f.spool, 'payout', name);
  fs.mkdirSync(dir, { recursive: true });
  const prefix = path.join(f.spool, 'working', name);
  const file = prefix + '.receipts.json';
  write(file, bundle);
  write(prefix + '.tap-rate.json', lock);
  fs.writeFileSync(prefix + '.settlement.raw.json', '');
  fs.writeFileSync(prefix + '.draft-retry', '');
  write(path.join(dir, 'tap.produced'), { schema_version: 1, rail: 'tap', status: 'produced',
    epoch: source.epoch, epoch_apply_hash: source.applyHash, evidence_sha256: digest(fs.readFileSync(file)) });
  const { sessionBindings, liabilities } = await resolveTargetedTapPayoutsFromLedger({ ...f, bundle });
  const [newEpoch, newAnchor] = [source.epoch + 1, hash(source.epoch + 1)];
  const recover = () => recoverDraft({ ...f, dir, name, action: 'fence', successor: { epoch: newEpoch, anchor: newAnchor } });
  await assert.rejects(recover(), /No confirmed TAP liability reservation/);
  f.pool.maxEpochDelta = f.pool.totalDeposited = async () => 100n * 10n ** 18n;
  const { default: ContractV30, payoutPreparationMessage } = await import('../../intercom/contract/v30.js');
  const historical = new ContractV30({ peer: { wallet: makeVerifier(signer.wallet) } }, {});
  const lostAck = async (input, options) => {
    if (options?.method !== 'POST') return f.fetchImpl(input, options);
    // Exercise the archived writer's actual wire signature and reducer on the
    // original economic plan. The transport loses the acknowledgement.
    const { value } = JSON.parse(options.body);
    value.contract_version = 30;
    value.admin_sig = Buffer.from(signer.wallet.sign(Buffer.from(payoutPreparationMessage(value)))).toString('hex');
    const key = await historical.targetedPayoutPreparationFeatureKey(value);
    const result = await executeFeature(historical, f.storage, 'mayhem_feature', key, value, signer.publicKey);
    assert.equal(result.ok, true, result.message);
    f.seal();
    throw new Error('Core accepted before disconnect');
  };
  let plan;
  await assert.rejects(rollTapSettlement({ pool: f.pool, peerRpcUrl: f.peerRpcUrl, fetchImpl: f.fetchImpl, bundle,
    targetedSessionBindings: sessionBindings, canonicalLiabilities: liabilities,
    tapUsdAu: lock.tap_usd_au, tapRateLock: lock, epochApplyHash: source.applyHash,
    ledgerFeeBps: 1500, payoutMinAu: (await source.f.ledger.activeParamsAt(at, ['payout_min_au'])).payout_min_au,
    settleThroughEpoch: source.epoch + 100, operatorAddress: addr(8),
    ownerSigner: { getAddress: async () => addr(6) },
    governanceSigner: { getAddress: async () => addr(7), signTypedData: async () => '0x' + '00'.repeat(65) },
    canonicalPreparationSubmitter: request => {
      plan = request.plan;
      return submitCanonicalTapPreparationPlan({ ...request, peerRpcUrl: f.peerRpcUrl, fetchImpl: lostAck,
        adminSigner: { publicKey: signer.publicKey, sign: msg => Buffer.from(signer.wallet.sign(Buffer.from(msg))).toString('hex') } });
    } }), /Core accepted before disconnect/);
  assert.equal(plan.preparation_ids.length, 2);
  const first = 'payout/preparation/tap/' + plan.preparation_ids[0];
  assert.equal(f.state.get(first).consumed, false);
  assert.equal(f.state.get(first).contract_version, 30);
  assert(!f.state.has('payout/preparation/tap/' + plan.preparation_ids[1]));
  assert(!f.state.has('payout/preparation/tap/' + plan.root_preparation_id));
  f.epoch(newEpoch);
  f.head.timestamp += 3000;
  const provider = f.pool.runner.provider;
  provider.getTransactionReceipt = () => assert.fail('a receipt cannot establish absence of broadcast');
  const block = provider.getBlock;
  provider.getBlock = async tag => tag === 'finalized' || tag === 4
    ? { number: 4, hash: '0x' + hash(4), timestamp: at + 1 } : block(tag);
  await assert.rejects(recover(), /authentic and expired/);
  provider.getBlock = block;
  const pending = f.pool.pendingRoot;
  f.pool.pendingRoot = async () => [plan.intent.root, BigInt(source.epoch), 2n, 0n, 0n, 0n, 999999n];
  await assert.rejects(recover(), /pending, executed or ambiguous/);
  f.pool.pendingRoot = pending;
  f.pool.merkleRootAtEpoch = async () => plan.intent.root;
  await assert.rejects(recover(), /pending, executed or ambiguous/);
  f.pool.merkleRootAtEpoch = async () => ZeroHash;
  const rate = f.state.get(lock.rate_record_key);
  f.state.delete(lock.rate_record_key); f.seal();
  await assert.rejects(recover(), /authentic and expired/);
  f.state.set(lock.rate_record_key, rate); f.seal();
  const before = f.storage.snapshotBytes();
  await recover();
  assert.equal(f.storage.snapshotBytes(), before);
  const archived = path.join(f.spool, 'deferred', name);
  assert.deepEqual(read(archived + '.draft.json').observed_liability_preparations, [plan.preparation_ids[0]]);
  const retained = read(archived + '.draft.json').debt_evidence;
  assert.deepEqual(retained.source.intent, plan.intent);
  assert.deepEqual(retained.source.preparation_ids, plan.preparation_ids);
  assert.equal(retained.opening_checkpoint, null);
  assert.deepEqual(retained.records.map(r => [r.economic_op_id, r.contract_version]), [[plan.preparation_ids[0], 30]]);
  assert.deepEqual(read(archived + '.receipts.json'), bundle);
  assert.deepEqual(read(archived + '.tap-rate.json'), lock);
  assert.equal(read(path.join(dir, 'tap.complete')).status, 'fenced');
  await assert.rejects(dispatchFencedTapTick({ ...f, payout: path.join(f.spool, 'payout'), adminSigner: {
    publicKey: signer.publicKey, sign: () => assert.fail('An uncommitted legacy remainder cannot be signed'),
  } }), /Partial historical TAP reservation lacks a signed full plan/);
  assert.equal(f.storage.snapshotBytes(), before);
});

for (const [mode, name] of [
  ['immutable', 'unattended timer replays the sole frozen provider, restores the mined quote and closes paid refund inputs'],
  ['alternative', 'unattended timer settles a repriced AU winner at the original epoch without other business'],
  ['original-wins', 'unattended timer reconciles the delayed original send after a competing AU offer'],
  ['prefix', 'unattended timer restores the signed v30 prefix after lost acknowledgement with no submission journal'],
]) test(name, async t => {
  const alternative = mode === 'alternative', legacy = mode === 'prefix', delayed = mode === 'original-wins';
  const signer = await makeIdentity();
  const f = fixture(t, signer);
  const eth = await canonicalPoolFixture(t);
  const { pool, provider } = eth;
  const adminSigner = { publicKey: signer.publicKey,
    sign: msg => Buffer.from(signer.wallet.sign(Buffer.from(msg))).toString('hex') };
  const quote = async price => {
    const ts = (await provider.getBlock('latest')).timestamp;
    const result = await executeRateFeature(f.reducer, f.storage, { op: 'tap_rate_oracle', source: 'uniswap-v3-twap',
      tap_usd_au: String(price), ts }, signer.publicKey);
    assert.equal(result.ok, true, result.message);
    f.seal();
  };
  if (!legacy) f.state.delete(liabilityKey(2));
  if (alternative || delayed) f.state.get(liabilityKey(1)).denom = 'au_usd';
  f.epoch(3);
  await quote(rateValue);
  const residue = { account: addr(10), amount_au: '1' };
  const work = await f.freeze(3, { refunds: [{ account: addr(9), amount_wei: '4' }, ...(!legacy ? [residue] : [])] });
  const { default: ContractV30, payoutPreparationMessage } = await import('../../intercom/contract/v30.js');
  const old = new ContractV30({ peer: { wallet: makeVerifier(signer.wallet) } }, {});
  const prepare = async (url, options) => {
    if (!legacy || options?.method !== 'POST') return f.fetchImpl(url, options);
    const { value } = JSON.parse(options.body);
    value.contract_version = 30;
    value.admin_sig = adminSigner.sign(payoutPreparationMessage(value));
    const key = await old.targetedPayoutPreparationFeatureKey(value);
    const accepted = await executeFeature(old, f.storage, 'mayhem_feature', key, value, signer.publicKey);
    assert.equal(accepted.ok, true, accepted.message);
    f.seal();
    throw Error('power loss after v30 Core write');
  };
  await assert.rejects(rollTapSettlement({ ...work.options, ...eth, post: true,
    canonicalPreparationSubmitter: req => submitCanonicalTapPreparationPlan({ ...work, ...req, adminSigner, fetchImpl: prepare,
      beforePost: legacy ? undefined : async () => {
        await sealTapSource(work.prefix, req.plan, req.openingCheckpoint, req.selectionBlock, adminSigner);
        journalTapPreparation(work.prefix + '.prepare-intent.json', req.plan);
      } }), beforeEthereumEffect() { throw Error('power loss'); } }), /power loss/);
  fs.writeFileSync(work.prefix + '.draft-retry', '');
  f.epoch(4);
  await eth.jump(2701);
  await quote(alternative ? rateValue * 2n : rateValue);
  await recoverDraft({ ...work, pool, action: 'fence', successor: { epoch: 4, anchor: hash(4) } });
  const archive = path.join(f.spool, 'deferred', work.name);
  const digestBefore = digest(fs.readFileSync(archive + '.draft.json'));
  const sealed = read(archive + '.draft.json').debt_evidence.source;
  const reservation = id => f.state.get(f.reducer.payoutPreparationRecordKey('tap', id));
  const prefix = structuredClone(reservation(sealed.preparation_ids[0]));
  let lose = false, intercept = delayed;
  const held = [];
  const transport = { send: async (method, params) => {
    if (intercept && method === 'eth_sendTransaction' &&
        params[0].data.startsWith(pool.interface.getFunction('proposeRoot').selector)) {
      held.push(structuredClone(params));
      throw Error('unknown transaction delivery');
    }
    return provider.send(method, params);
  } };
  // Every invocation starts with a fresh coordinator and independently reads
  // disk, Core and Ethereum. Repeated calls do not advance either clock.
  const tick = () => runDebtTimer({ spool: f.spool, payout: path.join(f.spool, 'payout'), peerRpcUrl: f.peerRpcUrl,
    ...eth, provider: transport, adminSigner, fetchImpl: async (u, o) => {
      const response = await f.fetchImpl(u, o);
      if (lose && o?.method === 'POST') { lose = false; throw Error('accepted feature, reply lost'); }
      return response;
    } });
  if (delayed) {
    await assert.rejects(tick(), /TAP debt recovery will retry/);
    await eth.jump(1);
    await quote(rateValue * 2n);
    assert.equal(await tick(), 'alternative submitted');
    await assert.rejects(tick(), /TAP debt recovery will retry/);
    assert.equal(held.length, 2);
    intercept = false;
    const signedRoots = held.map(([tx]) => pool.interface.parseTransaction(tx).args[0]);
    assert.equal(signedRoots[0], sealed.intent.root);
    assert.notEqual(signedRoots[1], sealed.intent.root);
    await provider.send('eth_sendTransaction', held[0]);
    assert.equal((await pool.pendingRoot())[0], sealed.intent.root);
    await assert.rejects(provider.call(held[1][0]));
  }
  if (legacy) {
    assert.equal(prefix.contract_version, 30);
    assert.equal(sealed.preparation_ids.length, 2);
    assert.equal(reservation(sealed.preparation_ids[1]), undefined);
    assert.equal(reservation(sealed.root_preparation_id), undefined);
    assert.equal(fs.existsSync(archive + '.prepare-intent.json'), false);
    lose = true;
    await assert.rejects(tick(), /accepted feature, reply lost/);
    assert.equal(reservation(sealed.preparation_ids[1]).contract_version, 31);
    assert.equal(await tick(), 'original preparation submitted');
    assert.deepEqual(reservation(sealed.preparation_ids[0]), prefix);
    assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 0);
  }
  if (alternative) {
    lose = true;
    await assert.rejects(tick(), /accepted feature, reply lost/);
    assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 0);
  }
  assert.equal(await tick(), 'governance delay');
  assert.equal(await tick(), 'governance delay');
  await eth.jump(3601);
  await quote(rateValue * 3n); // later prices cannot overwrite the mined selection
  assert.equal(await tick(), 'finality');
  for (let i = 0; i < 12; i++) await provider.send('evm_mine', []);
  assert.equal(await tick(), 'finalized');
  assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');
  assert.equal(reservation(sealed.root_preparation_id).consumed, false);
  lose = true;
  await assert.rejects(tick(), /accepted feature, reply lost/);
  assert.equal(await tick(), 'settled');
  assert.equal(await tick(), 'idle');
  const report = read(path.join(f.spool, 'processed/latest.json'));
  const cp = report.tap_settlement_checkpoint;
  if (alternative) {
    assert.equal(cp.epoch, sealed.intent.epoch);
    assert.notEqual(cp.root, sealed.intent.root);
    assert.notEqual(cp.root_preparation_id, sealed.root_preparation_id);
    assert.equal(cp.tap_rate_lock.tap_usd_au, String(rateValue * 2n));
    assert.equal(cp.proposal_rate_record_key, undefined);
    assert.equal(report.canonical_preparation_confirmed, false);
    const resolution = f.state.get('payout/recovery/tap/' + sealed.root_preparation_id + '/resolution');
    assert.equal(resolution.candidate_submission, report.conditional_authorization);
    assert.equal(resolution.root_preparation_id, cp.root_preparation_id);
    assert.equal(reservation(cp.root_preparation_id).consumed, true);
  } else assert(cp.proposal_rate_record_key && cp.proposal_rate_record_key !== cp.tap_rate_lock.rate_record_key);
  if (delayed) {
    assert.equal(cp.root, sealed.intent.root);
    assert.equal(cp.root_preparation_id, sealed.root_preparation_id);
    assert.equal(report.conditional_authorization, undefined);
    await assert.rejects(provider.call(held[1][0]));
    assert.equal(f.state.has('payout/recovery/tap/' + sealed.root_preparation_id + '/resolution'), false);
  }
  assert.equal(cp.outputs[0].paid_au, '101');
  assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '101');
  const amount = alternative ? 25n : 50n;
  assert.equal(report.proofs[addr(1)].cumulative_wei, String(amount));
  const claims = [[addr(1), amount], [addr(9), 4n]];
  if (legacy) {
    claims.push([addr(2), 6n]);
    assert.equal(reservation(sealed.preparation_ids[0]).admin_sig, prefix.admin_sig);
    assert.equal(reservation(sealed.preparation_ids[0]).contract_version, 30);
    assert.equal(reservation(sealed.preparation_ids[0]).consumed, true);
    assert.equal(f.state.get(liabilityKey(2)).paid_cum_au, '12');
  }
  for (const [account, wei] of claims) {
    await (await pool.claim(cp.epoch, account, wei, report.proofs[account].proof)).wait();
    assert.equal(await eth.token.balanceOf(account), wei);
    await assert.rejects(pool.claim.staticCall(cp.epoch, account, wei, report.proofs[account].proof));
  }
  assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 1);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted())).length, 1);
  assert.equal(digest(fs.readFileSync(archive + '.draft.json')), digestBefore);
  const next = await f.freeze(4);
  assert.deepEqual(next.bundle.tap_carried_refunds, legacy ? [] : [residue]);
  if (!legacy) {
    const fence = read(archive + '.draft.json');
    assert.deepEqual(fence.reserved_refund_credits, [{ account: addr(9), amount_wei: '4' }]);
    assert.deepEqual(fence.refunds, [residue]);
  }
  await verifyQueuedTapSweep(f.spool, next.bundle, pool);
  const carry = await rollTapSettlement({ ...next.options, ...eth, prior: report });
  assert.deepEqual(carry.refunds, cp.refunds);
  finishPage(next, carry, 'carry');
  const remaining = nextRefunds(f.spool, loadCursor(f.spool).refund_queue);
  assert.deepEqual(remaining?.entries, legacy ? undefined : [residue]);
});

for (const [mode, name] of [
  ['exact', 'timer pays disjoint held funds at their own price then substitutes the skipped debt on an empty page'],
  ['au', 'timer recovers the excluded original AU at a materially changed price and credits its buyer once'],
  ['history', 'timer certifies multiple disjoint intervening roots and pays the original at the next empty epoch'],
]) test(name, async t => {
  const reprice = mode !== 'exact', history = mode === 'history';
  const signer = await makeIdentity();
  const f = fixture(t, signer);
  const eth = await canonicalPoolFixture(t);
  const { pool, provider } = eth;
  const adminSigner = { publicKey: signer.publicKey,
    sign: msg => Buffer.from(signer.wallet.sign(Buffer.from(msg))).toString('hex') };
  const quote = async price => {
    const ts = (await provider.getBlock('latest')).timestamp;
    const result = await executeRateFeature(f.reducer, f.storage, { op: 'tap_rate_oracle', source: 'uniswap-v3-twap',
      tap_usd_au: String(price), ts }, signer.publicKey);
    assert.equal(result.ok, true, result.message);
    f.seal();
  };
  if (reprice) f.state.get(liabilityKey(1)).denom = 'au_usd';
  const bindingKey = 'payout/binding/tap/' + hash(2) + '/' + hash(2);
  const binding = f.state.get(bindingKey);
  f.state.delete(bindingKey);
  const third = { provider: hash(3), revision: hash(3), target: addr(3) };
  if (history) {
    f.state.set(liabilityKey(3), { ...f.state.get(liabilityKey(2)), ...third });
    f.state.set('earn/tap/' + hash(3), { ...f.state.get('earn/tap/' + hash(2)), ...third });
    f.state.set('prov/' + hash(3), { ...f.state.get('prov/' + hash(2)), provider: hash(3) });
  }
  f.epoch(3);
  await quote(rateValue);
  const work = await f.freeze(3, reprice ? { refunds: [{ account: addr(9), amount_wei: '4' }] } : {});
  const ordinary = (item, more = {}) => rollTapSettlement({ ...item.options, ...eth,
    post: true, canonicalPreparationSubmitter: req => submitCanonicalTapPreparationPlan({ ...item, ...req, adminSigner,
      beforePost: async () => {
        await sealTapSource(item.prefix, req.plan, req.openingCheckpoint, req.selectionBlock, adminSigner);
        journalTapPreparation(item.prefix + '.prepare-intent.json', req.plan);
      } }), ...more });
  await assert.rejects(ordinary(work, { beforeEthereumEffect() { throw Error('crash'); } }), /crash/);
  fs.writeFileSync(work.prefix + '.draft-retry', '');
  f.state.set(bindingKey, binding);
  f.epoch(4);
  await eth.jump(2701);
  await quote(rateValue * 2n);
  await recoverDraft({ ...work, pool, action: 'fence', successor: { epoch: 4, anchor: hash(4) } });
  let drop = null;
  const tick = () => runDebtTimer({ ...eth, adminSigner, spool: f.spool,
    payout: path.join(f.spool, 'payout'), peerRpcUrl: f.peerRpcUrl, fetchImpl: async (u, opts) => {
      const response = await f.fetchImpl(u, opts);
      if (opts?.method === 'POST' && JSON.parse(opts.body).value.op === drop) { drop = null; throw Error('lost ack'); }
      return response;
    } });
  if (!reprice) await assert.rejects(tick(), /source is not the original payable AU liability/);
  const fresh = await f.freeze(4);
  assert.equal(await dispatchFencedTapTick({ ...f, ...eth, adminSigner, payout: path.join(f.spool, 'payout') }), 'ordinary work');
  assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 0);
  const payDisjoint = async (item, prior) => {
    const run = () => ordinary(item, { prior });
    assert.equal((await run()).awaiting_governance_delay, true);
    await eth.jump(3601);
    assert.equal((await run()).awaiting_finality, true);
    for (let i = 0; i < 12; i++) await provider.send('evm_mine', []);
    const final = await run();
    const cp = final.tap_settlement_checkpoint;
    const normal = await executeFeature(f.reducer, f.storage, 'mayhem_feature',
      await f.reducer.targetedTapSettlementFeatureKey(cp), cp, signer.publicKey);
    assert.equal(normal.ok, true, normal.message);
    f.seal();
    write(path.join(f.spool, 'processed/latest.json'), final);
    finishPage(item, final, 'settled');
    return final;
  };
  let latest = await payDisjoint(fresh, null);
  await (await pool.claim(4, addr(2), 3n, latest.proofs[addr(2)].proof)).wait();
  const targetEpoch = history ? 6 : 5;
  if (history) {
    f.state.set('payout/binding/tap/' + hash(3) + '/' + hash(3), { ...binding, ...third });
    f.epoch(5);
    await quote(rateValue * 2n);
    latest = await payDisjoint(await f.freeze(5), latest);
    await (await pool.claim(5, addr(3), 3n, latest.proofs[addr(3)].proof)).wait();
  }
  f.epoch(targetEpoch);
  await quote(reprice ? rateValue * 2n : 201n * 10n**16n);
  const empty = await f.freeze(targetEpoch);
  const carry = await ordinary(empty, { prior: latest });
  assert.equal(carry.reason, 'no payable TAP liabilities on this page');
  finishPage(empty, carry, 'carry');
  if (history) {
    drop = 'recover_prepared_tap';
    await assert.rejects(tick(), /lost ack/);
    assert.equal(await tick(), 'historical checkpoint confirmed');
    assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '0');
  }
  drop = 'recover_prepared_tap';
  await assert.rejects(tick(), /lost ack/);
  assert.equal(await tick(), 'governance delay');
  assert.equal(await tick(), 'governance delay');
  await eth.jump(3601);
  assert.equal(await tick(), 'finality');
  for (let i = 0; i < 12; i++) await provider.send('evm_mine', []);
  assert.equal(await tick(), 'finalized');
  assert.equal(await tick(), 'checkpoint submitted');
  assert.equal(await tick(), 'settled');
  assert.equal(await tick(), 'idle');
  const paid = read(path.join(f.spool, 'processed/latest.json'));
  assert.equal(paid.tap_settlement_checkpoint.outputs[0].paid_au, '101');
  assert.equal(await pool.merkleRootAtEpoch(3), ZeroHash);
  const originalWei = reprice ? 25n : 50n;
  await (await pool.claim(targetEpoch, addr(1), originalWei, paid.proofs[addr(1)].proof)).wait();
  if (reprice) {
    await (await pool.claim(targetEpoch, addr(9), 4n, paid.proofs[addr(9)].proof)).wait();
    assert.equal(await eth.token.balanceOf(addr(9)), 4n);
    await assert.rejects(pool.claim.staticCall(targetEpoch, addr(9), 4n, paid.proofs[addr(9)].proof));
    f.epoch(targetEpoch + 1);
    const next = await f.freeze(targetEpoch + 1);
    assert.equal(next.bundle.tap_carried_refunds, undefined);
  }
  for (const [account, amount] of [[addr(1), originalWei], [addr(2), 3n]]) {
    assert.equal(await eth.token.balanceOf(account), amount);
    await assert.rejects(pool.claim.staticCall(targetEpoch, account, amount, paid.proofs[account].proof));
  }
  assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, history ? 3 : 2);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted())).length, history ? 3 : 2);
  if (history) {
    assert.equal(await eth.token.balanceOf(addr(3)), 3n);
    await assert.rejects(pool.claim.staticCall(targetEpoch, addr(3), 3n, paid.proofs[addr(3)].proof));
  }
  assert.equal(f.state.get(liabilityKey(1)).paid_cum_au, '101');
  assert.equal(f.state.get(liabilityKey(2)).paid_cum_au, '12');
});
