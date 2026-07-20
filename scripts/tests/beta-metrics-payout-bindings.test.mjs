import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const metricsScript = path.join(repoRoot, 'scripts/beta-metrics.mjs');
const template = JSON.parse(fs.readFileSync(
  path.join(repoRoot, 'config/beta/metrics.template.json'),
  'utf8',
));

function validate(metrics) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-beta-metrics-payout-'));
  const metricsPath = path.join(tmp, 'metrics.json');
  fs.writeFileSync(metricsPath, `${JSON.stringify(metrics, null, 2)}\n`);
  const child = spawnSync(process.execPath, [
    metricsScript,
    '--metrics',
    metricsPath,
    '--allow-placeholders',
    '--json',
  ], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  fs.rmSync(tmp, { recursive: true, force: true });
  assert.equal(child.signal, null, child.stderr);
  return {
    child,
    report: JSON.parse(child.stdout),
  };
}

test('beta metrics template requires provider-owned payout binding evidence', () => {
  const current = validate(template);
  assert.equal(current.child.status, 0, current.report.errors.join('\n'));
  assert.equal(current.report.ok, true);

  const retired = structuredClone(template);
  delete retired.controls.provider_payout_bindings_permissionless;
  delete retired.controls.provider_payout_bindings_ownership_verified;
  retired.controls.admin_sets_provider_payout_targets = true;
  retired.controls.provider_payout_targets_admin_verified = true;
  delete retired.canonical_service.provider_payout_bindings_verified;
  retired.canonical_service.admin_payout_records_verified = true;

  const rejected = validate(retired);
  assert.equal(rejected.child.status, 1);
  const errors = rejected.report.errors.join('\n');
  assert.match(errors, /admin_sets_provider_payout_targets is retired/);
  assert.match(errors, /provider_payout_targets_admin_verified is retired/);
  assert.match(errors, /admin_payout_records_verified is retired/);
  assert.match(errors, /provider_payout_bindings_permissionless must be true/);
  assert.match(errors, /provider_payout_bindings_verified must be true/);
});
