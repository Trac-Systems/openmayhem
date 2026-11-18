import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const source = fileURLToPath(new URL('../../', import.meta.url));

test('payout workers execute their CLI through a symlinked release directory', t => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'payout-entry-'));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const installed = path.join(temp, 'installed release');
  fs.symlinkSync(source, installed, 'dir');
  function run(script, args = []) {
    return spawnSync(process.execPath, [path.join(installed, script), ...args], {
      env: { PATH: process.env.PATH }, cwd: temp, encoding: 'utf8', timeout: 10000,
    });
  }

  for (const [script, error] of [
    ['scripts/mainnet-proof.mjs', /Ethereum RPC is required/],
    ['scripts/validate-tap-settlement-checkpoint.mjs', /path|argument/i],
    ['contracts/scripts/tap-settlement-roller.mjs', /Missing --bundle/],
    ['contracts/scripts/tap-deposit-watcher.mjs', /Missing --rpc/],
  ]) {
    const result = run(script);
    assert.equal(result.status, 1, script);
    assert.match(result.stderr, error);
  }

  const bundle = path.join(temp, 'bundle.json');
  const output = path.join(temp, 'tap.json');
  fs.writeFileSync(bundle, JSON.stringify({epoch: 3, receipts: []}));
  const applyHash = 'a'.repeat(64);
  const result = run('intercom/scripts/payout-tap-receipts.mjs', [bundle, output, '3', applyHash]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), '0');
  const written = JSON.parse(fs.readFileSync(output, 'utf8'));
  assert.equal(written.epoch_apply_hash, applyHash);
  assert.deepEqual(written.receipts, []);
});
