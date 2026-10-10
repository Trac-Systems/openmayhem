import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { proxyPayoutEpoch, matureFixtureLiabilities } from '../../intercom/tests/helpers/proxy-payout-epoch.mjs';

import { installPayoutDouble } from './helpers/mixed-payout-cli.mjs';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const sha256 = value => createHash('sha256').update(value).digest('hex');
function writeJson(file, value) { const bytes = JSON.stringify(value) + '\n'; fs.writeFileSync(file, bytes, { mode: 0o600 }); return bytes; }
function harness(t, fixture) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-proxy-payout-'));
  fs.chmodSync(root, 0o700); t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bin = path.join(root, 'bin'), state = path.join(root, 'settlement'), spool = path.join(state, 'tap');
  const epochDir = path.join(state, `epochs/epoch-${fixture.epoch}`);
  fs.mkdirSync(bin); fs.mkdirSync(epochDir, { recursive: true });
  const bundle = writeJson(path.join(epochDir, 'epoch-bundle.json'), fixture.bundle);
  const recomputed = writeJson(path.join(epochDir, 'epoch-recomputed.json'), fixture.recomputed);
  const receipts = writeJson(path.join(epochDir, 'canonical-receipts.json'), fixture.bundle.receipt_snapshot);
  writeJson(path.join(epochDir, 'epoch-artifact.json'), { schema_version: 1, type: 'canonical_epoch_artifact',
    rail: 'all', rails: ['fiat', 'tap', 'tnk'], epoch: fixture.epoch, epoch_apply_hash: fixture.applyHash,
    bundle_sha256: sha256(bundle), recomputed_sha256: sha256(recomputed), canonical_receipts_sha256: sha256(receipts),
    roots: fixture.recomputed.roots, totals: fixture.recomputed.totals });
  const records = path.join(root, 'records.json'); writeJson(records, fixture.records);
  // Only the RPC transport is doubled. Every returned record was produced by
  // the actual contract above; unknown requests fail instead of reaching a host.
  fs.writeFileSync(path.join(bin, 'curl'), `#!${process.execPath}
const fs = require('node:fs');
const url = new URL(process.argv.at(-1));
if (url.origin !== 'http://mock.invalid' || url.pathname !== '/v1/state') process.exit(91);
const key = url.searchParams.get('key'), records = JSON.parse(fs.readFileSync(process.env.FIXTURE_RECORDS));
if (!(key in records)) process.exit(92);
process.stdout.write(JSON.stringify(records[key]));
`, { mode: 0o700 });
  fs.writeFileSync(path.join(bin, 'no-financial-cli'), '#!/bin/sh\nexit 93\n', { mode: 0o700 });
  const env = { PATH: `${bin}:${path.dirname(process.execPath)}:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: path.join(root, 'home'), LANG: 'C',
    MAYHEM_BIN: path.join(bin, 'no-financial-cli'), MAYHEM_SOURCE_DIR: ROOT, MAYHEM_RPC_URL: 'http://mock.invalid/v1',
    MAYHEM_ADMIN_HOME: path.join(root, 'admin-home'), MAYHEM_ADMIN_STORE: 'synthetic-only', MAYHEM_CADENCE_STATE_DIR: state,
    MAYHEM_TAP_SETTLEMENT_SPOOL: spool, MAYHEM_PAYOUT_LOCK_HELD: '1', MAYHEM_PAYOUT_TEST_MODE: '1', MAYHEM_PAYOUT_TEST_ROOT: root,
    MAYHEM_FIAT_SETTLEMENT_ENABLED: '0', MAYHEM_TNK_SETTLEMENT_ENABLED: '0', MAYHEM_TAP_SETTLEMENT_ENABLED: '1', FIXTURE_RECORDS: records };
  return { root, state, spool, env, records, epochDir, run: () => spawnSync('/bin/bash', [path.join(ROOT, 'scripts/ops-payout-settle.sh')],
    { cwd: ROOT, env, encoding: 'utf8', timeout: 20_000, maxBuffer: 256 * 1024 }) };
}

for (const mixed of [false, true]) for (const rail of ['fiat', 'tnk', 'tap']) test(`actual ${rail} ${mixed ? 'mixed native/proxy' : 'proxy'} epoch reaches the unchanged TAP worker without rewriting signed receipts`, async t => {
  const fixture = await proxyPayoutEpoch(rail, { mixed }), ctx = harness(t, fixture);
  assert.equal(fixture.bundle.receipts.find(head => head.lane === 'proxy').receipt.body.rail, undefined, 'proxy rail is bound by signed accepted terms');
  const result = ctx.run();
  assert.equal(result.status, 0, result.stderr);
  const queued = fs.readdirSync(path.join(ctx.spool, 'ready'));
  assert.equal(queued.length, rail === 'tap' ? 1 : 0);
  if (rail === 'tap') {
    const retained = JSON.parse(fs.readFileSync(path.join(ctx.spool, 'ready', queued[0])));
    assert.deepEqual(retained.receipts.map(head => head.receipt), fixture.bundle.receipts.map(head => head.receipt));
    assert.deepEqual(retained.proxy_acceptances, fixture.bundle.proxy_acceptances);
  }
});

for (const rail of ['fiat', 'tnk']) test(`real mixed ${rail} liabilities survive worker restart after external transfer before ACK`, async t => {
  const fixture = await proxyPayoutEpoch(rail, { mixed: true });
  await matureFixtureLiabilities(fixture);
  const ctx = harness(t, fixture), external = await installPayoutDouble(ctx, fixture, rail);
  assert.equal(external.outputs.length, 2);
  const first = ctx.run(); assert.notEqual(first.status, 0);
  const effects = () => fs.readFileSync(path.join(external.effects, 'sent.log'), 'utf8').trim().split('\n');
  assert.equal(effects().length, 1, 'fixture stopped after the transfer, before the report');
  const second = ctx.run(); assert.equal(second.status, 0, second.stderr);
  assert.equal(effects().length, 2); assert.equal(new Set(effects()).size, 2);
  const work = path.join(ctx.state, `payout/epoch-${fixture.epoch}-${fixture.applyHash}`);
  assert.ok(fs.existsSync(path.join(work, `${rail}.complete`)));
  const third = ctx.run(); assert.equal(third.status, 0, third.stderr);
  assert.equal(effects().length, 2);
  for (const output of external.outputs) assert.deepEqual(JSON.parse(fs.readFileSync(path.join(external.effects, output.economic_op_id + '.effect'))), output);
});

for (const corruption of ['wrong-rail', 'signature']) test(`invalid proxy ${corruption} cannot strand valid native FIAT payout in the rail worker`, async t => {
  const fixture = await proxyPayoutEpoch('fiat', { mixed: true });
  await matureFixtureLiabilities(fixture);
  // Corrupt the retained proxy artifact only. The actual contract liabilities
  // remain unchanged and valid. The shell must isolate the failing TAP spool
  // from independently reconciled FIAT liabilities, never skip a bad TAP item.
  const head = fixture.bundle.receipts.find(head => head.lane === 'proxy');
  if (corruption === 'wrong-rail') head.rail = 'tap';
  else head.receipt.provider_sig = '0'.repeat(128);
  const ctx = harness(t, fixture), external = await installPayoutDouble(ctx, fixture, 'fiat');
  ctx.env.FIXTURE_CRASH = '0';
  const run = ctx.run(); assert.notEqual(run.status, 0, 'invalid TAP extraction must remain an error');
  const work = path.join(ctx.state, `payout/epoch-${fixture.epoch}-${fixture.applyHash}`);
  assert.ok(fs.existsSync(path.join(work, 'fiat.complete')), run.stderr);
  assert.ok(!fs.existsSync(path.join(work, 'tap.complete')));
  assert.equal(fs.readdirSync(external.effects).filter(name => name.endsWith('.effect')).length, 2);
  assert.equal(fs.readdirSync(path.join(ctx.spool, 'ready')).length, 0);
});
