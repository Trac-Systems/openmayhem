import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { verifyTapSweepReport } from '../scripts/verify-tap-sweep-report.mjs';

const hash = '1'.repeat(64);
const provider = '2'.repeat(64);
const revision = '3'.repeat(64);
const target = '0x' + '4'.repeat(40);
const next = `payout/liability/tap/${provider}/${revision}`;
const emptyReason = 'no payable TAP liabilities on this page';
function fixture() {
  const selector = { schema_version: 1, signed_length: 23, after: null };
  const bundle = { epoch: 8, epoch_apply_hash: hash, rail: 'tap', receipts: [], params: { fee_bps: 1500 },
    at: 940, tap_liability_sweep: selector };
  const rateLock = { epoch: 8, rate_ts: 910, tap_usd_au: '1000000000000000000', chain_id: 1 };
  const report = { epoch: 8, epoch_apply_hash: hash, posted: false, reason: emptyReason, spent_au: '0',
    held_receipt_count: 0, receipt_count: 0, held_au: '0', threshold_held_au: '0',
    threshold_held_provider_count: 0, canonical_deferred_liabilities: [], checkpoint_outputs: [],
    tap_usd_au: rateLock.tap_usd_au, tap_rate_lock: structuredClone(rateLock),
    ledger_fee_bps: 1500, payout_min_au: '1',
    payout_minimum_lock: { type: 'tap_payout_minimum_lock', key: 'params/payout_min_au',
      value: '1', at: 940, signed_length: 23 },
    tap_liability_sweep: { ...selector, at: 940, eligible_count: 0, next_after: null } };
  return { bundle, report, rateLock, peerRpcUrl: 'http://fixture.invalid/v1',
    fetchImpl: stateReader() };
}

function emptyState() {
  const admin = '5'.repeat(64);
  return {
    admin,
    'epoch/apply/state': { updated_epoch: 8, last_apply_hash: hash, pending_epoch: null, updated_at: 'tx', last_settlement_unix: 900 },
    'epoch/apply-anchor/8': { type: 'epoch_apply_anchor', epoch: 8, apply_hash: hash, settlement_unix: 900, applied_at: 'tx' },
    'params/payout_min_au': { key: 'payout_min_au', current: { ver: 0, effective_at: 0, value: '1' }, pending: null },
    'payments/current': { ver: 1, rails: ['tap'], denom: 'au_usd', set_by: admin, set_by_role: 'admin',
      tnk: { network: 'mainnet' }, tap: { chain_id: 1,
        token_address: '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454', pool_address: '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28' } },
  };
}

function stateReader({ eligible = false, after = null, responseLength = 23, observe = () => {} } = {}) {
  const values = emptyState();
  const row = { type: 'provider_payout_liability', provider, rail: 'tap', revision, target, currency: null,
    chain_id: 1, updated_epoch: 8, total_au: '16', paid_cum_au: '4', held_au: '2',
    holdbacks: [{ epoch: 8, au: '2', locked_epochs: 3 }] };
  values[`payout/binding/tap/${provider}/${revision}`] = {
    type: 'provider_payout_binding', verified: true, provider, rail: 'tap', revision, target,
    currency: null, chain_id: 1, network: 'mainnet', activation_epoch: 1, payment_config_version: 1,
  };
  values[`prov/${provider}`] = { provider, status: 'active' };
  values[`earn/tap/${provider}`] = { rail: 'tap', provider, denom: 'au_usd', total_au: '19',
    paid_cum_au: '7', held_au: '2', updated_epoch: 8, holdbacks: row.holdbacks,
    chain_id: 1, pool_address: values['payments/current'].tap.pool_address };
  return async url => {
    const params = url.searchParams;
    assert.equal(params.get('signed_length'), '23');
    assert.equal(params.get('confirmed'), 'true');
    const key = params.get('key');
    let record;
    if (key === null) {
      assert.equal(params.get('after'), after);
      observe();
      record = { prefix: params.get('prefix'), values: eligible ? [{ key: next, value: row }] : [],
        truncated: false, next_cursor: null };
    } else {
      record = { key, value: values[key] ?? null };
    }
    return { ok: true, json: async () => ({ ...record, confirmed: true, signed_length: responseLength }) };
  };
}

test('empty receipt list can carry only after the canonical resolver reads the pinned page again', async () => {
  const ctx = fixture();
  let pageReads = 0;
  ctx.fetchImpl = stateReader({ observe: () => pageReads++ });
  await verifyTapSweepReport(ctx);
  await verifyTapSweepReport(ctx);
  assert.equal(pageReads, 2);
  ctx.fetchImpl = stateReader({ responseLength: 24 });
  await assert.rejects(verifyTapSweepReport(ctx), /signed length/);
});

test('rejects sweep downgrades, invented work, mismatched epoch, timing, locks or cursor', async () => {
  const changes = [
    ctx => { delete ctx.bundle.tap_liability_sweep; },
    ctx => { delete ctx.report.tap_liability_sweep; },
    ctx => { delete ctx.report.epoch_apply_hash; },
    ctx => { ctx.report.epoch++; },
    ctx => { ctx.bundle.tap_liability_sweep.signed_length = true; },
    ctx => { ctx.report.receipt_count = 1; },
    ctx => { ctx.report.tap_liability_sweep.after = next; },
    ctx => { ctx.report.tap_liability_sweep.eligible_count = false; },
    ctx => { ctx.report.tap_liability_sweep.next_after = next; },
    ctx => { ctx.report.tap_liability_sweep.at = ctx.rateLock.rate_ts; },
    ctx => { ctx.report.payout_minimum_lock.at = ctx.rateLock.rate_ts; },
    ctx => { ctx.report.payout_minimum_lock.value = '0'; },
    ctx => { ctx.report.payout_minimum_lock.signed_length++; },
    ctx => { ctx.report.ledger_fee_bps++; },
    ctx => { ctx.bundle.params.payout_min_au = '100'; },
    ctx => { ctx.report.tap_rate_lock.rate_ts++; },
    ctx => { ctx.report.reason = 'provider earnings are below payout minimum'; },
    ctx => { ctx.report.held_receipt_count = 1; },
    ctx => { ctx.report.execution_tx = hash; },
    ctx => { ctx.report.burn = { auto_sent: true }; },
    ctx => { ctx.report.tap_settlement_checkpoint = {}; },
    ctx => { ctx.report.spent_au = '1'; },
    ctx => { ctx.report.checkpoint_outputs.push({}); },
    ctx => { ctx.fetchImpl = stateReader({ eligible: true }); },
  ];
  for (const change of changes) {
    const ctx = fixture();
    change(ctx);
    await assert.rejects(verifyTapSweepReport(ctx), undefined, String(change));
  }
});

test('absent bundle time uses the retained rate timestamp and retries the same exclusive cursor', async () => {
  const ctx = fixture();
  delete ctx.bundle.at;
  ctx.bundle.tap_liability_sweep.after = next;
  ctx.report.tap_liability_sweep.after = next;
  ctx.report.tap_liability_sweep.at = ctx.report.payout_minimum_lock.at = ctx.rateLock.rate_ts;
  ctx.fetchImpl = stateReader({ after: next });
  await verifyTapSweepReport(ctx);
});

test('eligible dust requires the exact below-wei evidence even on a zero-receipt epoch', async () => {
  const ctx = fixture();
  ctx.fetchImpl = stateReader({ eligible: true });
  ctx.rateLock.tap_usd_au = ctx.report.tap_usd_au = ctx.report.tap_rate_lock.tap_usd_au = '100000000000000000000';
  ctx.report.tap_liability_sweep.eligible_count = 1;
  Object.assign(ctx.report, { reason: 'provider earnings await challenge or holdback maturity',
    held_receipt_count: 1, held_au: '12', threshold_held_provider_count: 1, threshold_held_au: '10',
    canonical_deferred_liabilities: [{ provider, payout_revision: revision, to: target,
      payable_au: '10', reason: 'below_tap_wei_precision' }] });
  await verifyTapSweepReport(ctx);
  ctx.report.reason = emptyReason;
  await assert.rejects(verifyTapSweepReport(ctx), /reason/);
  ctx.report.reason = 'provider earnings await challenge or holdback maturity';
  ctx.report.canonical_deferred_liabilities[0].payable_au = '9';
  await assert.rejects(verifyTapSweepReport(ctx), /pinned liabilities/);
});

test('matured page binds exact AU, TAP wei and paid watermarks in both output representations', async () => {
  const ctx = fixture();
  ctx.fetchImpl = stateReader({ eligible: true });
  ctx.report.tap_liability_sweep.eligible_count = 1;
  delete ctx.report.reason;
  Object.assign(ctx.report, { held_receipt_count: 1, held_au: '2', spent_au: '10',
    checkpoint_outputs: [{ role: 'provider', provider, payout_revision: revision, to: target,
      liability_total_au: '16', held_au: '2', paid_cum_au_before: '4', aggregate_paid_cum_au_before: '7',
      net_au_paid: '10', tap_wei: '10', previous_cumulative_wei: '5', new_cumulative_wei: '15' }],
    tap_settlement_checkpoint: { at: 940, provider_paid_au: '10', fee_bps: 1500, outputs: [{
      provider, payout_revision: revision, to: target, paid_au: '10', tap_wei: '10',
      paid_cum_au_before: '4', aggregate_paid_cum_au_before: '7',
      prior_cumulative_claim_wei: '5', cumulative_claim_wei: '15' }] } });
  await verifyTapSweepReport(ctx);
  await assert.rejects(verifyTapSweepReport({ ...ctx, confirmed: true }), /confirmed Core preparation/);
  ctx.report.tap_settlement_checkpoint.outputs[0].to = '0x' + '0'.repeat(40);
  await assert.rejects(verifyTapSweepReport(ctx), /checkpoint/);
  delete ctx.report.tap_settlement_checkpoint;
  ctx.report.checkpoint_outputs[0].paid_cum_au_before = '0';
  await assert.rejects(verifyTapSweepReport(ctx), /pinned liability/);
});

test('legacy nonempty spool does not read a sweep, empty legacy spool cannot pass', async () => {
  const ctx = { bundle: { receipts: [{}] }, report: {}, fetchImpl: assert.fail };
  await verifyTapSweepReport(ctx);
  ctx.bundle.receipts = [];
  await assert.rejects(verifyTapSweepReport(ctx), /page proof/);
});

test('spool binding and completion each read the pinned page; interrupted reads leave the item retryable', t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-sweep-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const wrapper = fs.readFileSync(path.join(repo, 'scripts/ops/run-tap-settlement-roller.sh'), 'utf8');
  const worker = fs.readFileSync(path.join(repo, 'scripts/ops-payout-settle.sh'), 'utf8');
  const helpers = wrapper.slice(wrapper.indexOf('publish_json()'), wrapper.indexOf('while true; do'));
  const binding = wrapper.slice(wrapper.indexOf('  if ! verify_sweep "$dry_report"'),
    wrapper.indexOf("  if ! jq -e '\n    .blocked != true"));
  const completion = worker.slice(worker.indexOf('validate_tap_no_work()'), worker.indexOf('validate_tap_cli_report()'));
  assert.ok(binding.includes('publish_json "$bound_report"'));
  const ctx = fixture();
  Object.assign(ctx.report, { entries: [], providers: [], refunds: [], cumulative_spent_wei: '0' });
  const working = path.join(root, 'working');
  const processed = path.join(root, 'processed');
  fs.mkdirSync(working);
  fs.mkdirSync(processed);
  const name = `epoch-8-${hash}`;
  const receiptName = `${name}.receipts.json`;
  const reportName = `${name}.settlement.json`;
  const json = (name, value) => fs.writeFileSync(name, JSON.stringify(value));
  json(path.join(working, receiptName), ctx.bundle);
  json(path.join(working, 'dry'), ctx.report);
  json(path.join(working, `${name}.tap-rate.json`), ctx.rateLock);
  const preload = path.join(root, 'fixture.mjs');
  fs.writeFileSync(preload, `
    import fs from 'node:fs';
    const state = ${JSON.stringify(emptyState())};
    globalThis.fetch = async url => {
      if (url.hostname !== 'fixture.invalid' || url.searchParams.get('signed_length') !== '23' ||
          url.searchParams.get('confirmed') !== 'true') throw new Error('unexpected request');
      const key = url.searchParams.get('key');
      if (key === null) fs.appendFileSync(process.env.READS, 'page\\n');
      const result = key === null ? { prefix: 'payout/liability/tap/', truncated: false, values: [], next_cursor: null } :
        { key, value: state[key] ?? null };
      return Response.json({ ...result, confirmed: true, signed_length: Number(process.env.RESPONSE_PIN) });
    };`);
  const reads = path.join(root, 'reads');
  const env = { PATH: process.env.PATH, NODE_OPTIONS: `--import=${preload}`, READS: reads,
    repo, SOURCE_DIR: repo, TAP_SPOOL: root, working, processed, name, applied_epoch: '8', apply_hash: hash,
    peer_rpc: ctx.peerRpcUrl, RPC_URL: ctx.peerRpcUrl, attempt_timeout: '3', interval: '0',
    bundle: path.join(working, receiptName), rate_lock: path.join(working, `${name}.tap-rate.json`),
    dry_report: path.join(working, 'dry'), bound_report: path.join(working, 'bound'),
    raw_report: path.join(working, 'raw'), final_report: path.join(working, 'final'),
    event_cursor: path.join(working, 'events'), buyer_accounts: path.join(working, 'buyers') };
  const run = (source, pin) => spawnSync('bash', ['-eu', '-c', source], {
    env: { ...env, RESPONSE_PIN: String(pin) }, encoding: 'utf8' });
  const bind = `timeout() { shift; "$@"; }
${helpers}
for attempt in once; do
${binding}
done`;
  assert.equal(run(bind, 24).status, 0); // A failed attempt leaves the service loop running.
  assert.equal(fs.existsSync(env.bundle), true);
  assert.equal(fs.existsSync(path.join(processed, reportName)), false);
  const good = run(bind, 23);
  assert.equal(good.status, 0, good.stderr);
  const archived = JSON.parse(fs.readFileSync(path.join(processed, reportName)));
  assert.equal(archived.receipt_count, 0);
  assert.equal(archived.status, 'no_work');
  assert.equal(fs.existsSync(path.join(processed, 'latest.json')), false);
  const finish = `
json_field() { jq -r --arg key "$1" '.[$key] // empty'; }
mark_rail() { echo "$1 $2" >"$TAP_SPOOL/complete"; }
${completion}
complete_tap_processed "$processed/$name.settlement.json" "$processed/$name.receipts.json"`;
  assert.notEqual(run(finish, 24).status, 0);
  assert.equal(fs.existsSync(path.join(root, 'complete')), false);
  const result = run(finish, 23);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(fs.readFileSync(path.join(root, 'complete'), 'utf8'), 'tap carry\n');
  assert.equal(fs.readFileSync(reads, 'utf8'), 'page\npage\n');
});

test('finalized zero-receipt roller report passes both existing payment gates on replay', async () => {
  const { rollTapSettlement } = await import('../scripts/tap-settlement-roller.mjs');
  const { checkTapSpoolReport, checkTapProcessedReport } = await import('./helpers/tap-spool-report.mjs');
  const { stableJson } = await import('../../intercom/scripts/recompute-epoch-roots.mjs');
  const { createHash } = await import('node:crypto');
  const ctx = fixture();
  ctx.fetchImpl = stateReader({ eligible: true });
  ctx.bundle.params.payout_min_au = '1';
  Object.assign(ctx.rateLock, emptyState()['payments/current'].tap, {
    type: 'tap_settlement_rate_lock', denom: 'tap_usd_au', source: 'fixture',
    bundle_sha256: createHash('sha256').update(stableJson(ctx.bundle)).digest('hex'),
    rate_record_key: 'rate/tap/910/' + hash, posted_by: hash, posted_by_role: 'admin', payment_config_ver: 1,
  });
  const args = { bundle: ctx.bundle, epochApplyHash: hash, tapRateLock: ctx.rateLock,
    tapUsdAu: ctx.rateLock.tap_usd_au, targetedSessionBindings: {}, canonicalLiabilities: [],
    peerRpcUrl: ctx.peerRpcUrl, fetchImpl: ctx.fetchImpl, settleThroughEpoch: 8 };
  const draft = await rollTapSettlement({ ...args, post: false });
  const pool = {
    epoch: async () => 8, cumulativeSpent: async () => BigInt(draft.cumulative_spent_wei),
    merkleRoot: async () => draft.root, pendingRoot: async () => ({}),
    getAddress: async () => ctx.rateLock.pool_address, totalDeposited: async () => 1000n,
    maxEpochDelta: async () => 1000n, operatorWithdrawn: async () => 2n, totalBurned: async () => 2n,
    operatorClaimable: async () => 0n, burnClaimable: async () => 0n,
    filters: { RootPosted: true, RootProposed: true }, queryFilter: assert.fail,
    runner: { provider: { getNetwork: async () => ({ chainId: 1n }), getBlock: async () => ({ number: 30 }) } },
  };
  const report = await rollTapSettlement({ ...args, pool, post: true,
    ownerSigner: { getAddress: async () => target },
    rootEventReader: async ({ name }) => ({ args: { nonce: 1n, executeAfter: 900n }, blockNumber: name === 'RootPosted' ? 15 : 10,
      blockHash: '0x' + hash, transactionHash: '0x' + (name === 'RootPosted' ? provider : revision),
      getTransactionReceipt: async () => ({ status: 1 }) }),
    canonicalPreparationSubmitter: async ({ plan }) => ({
      all_existing: true, preparation_ids: plan.preparation_ids, root_preparation_id: plan.root_preparation_id,
      external_effect_ids: plan.external_effect_ids, records: plan.preparations.map(entry => ({
        type: 'targeted_payout_preparation', economic_op_id: entry.economic_op_id, consumed: false })),
    }) });
  report.payout_minimum_lock = ctx.report.payout_minimum_lock;
  delete report.dist;
  assert.equal(report.receipt_count, 0);
  assert.equal(report.root_already_posted, true);
  assert.equal(report.canonical_preparation_confirmed, true);
  await verifyTapSweepReport({ ...ctx, report, confirmed: true });
  const spool = checkTapSpoolReport(ctx.bundle, report);
  assert.equal(spool.finalizable, true, spool.error || spool.gateError);
  const result = checkTapProcessedReport(ctx.bundle, spool.bound);
  assert.equal(result.status, 0, result.error);
  report.preparation_ids = [];
  await assert.rejects(verifyTapSweepReport({ ...ctx, report, confirmed: true }), /preparation_ids/);
});
