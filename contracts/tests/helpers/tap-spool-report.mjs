import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Execute the production wrapper's binding and completion gate against real
// roller reports. No payment transport or filesystem lifecycle is mocked here.
export function checkTapSpoolReport(bundle, report) {
  const wrapper = fs.readFileSync(new URL('../../../scripts/ops/run-tap-settlement-roller.sh', import.meta.url), 'utf8');
  const binding = wrapper.match(/python3 - "\$bundle" "\$raw_report" "\$bound_report" <<'PY'\n([\s\S]*?)\nPY/);
  const completion = wrapper.match(/if ! jq -e '\n(\s+\.blocked != true and\n\s+\.root_confirmed == true[\s\S]*?)\n  ' "\$final_report"/);
  assert.ok(binding && completion, 'production report binding and completion gate exist');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-spool-report-'));
  try {
    const paths = ['bundle', 'raw', 'bound'].map(name => path.join(root, `${name}.json`));
    fs.writeFileSync(paths[0], JSON.stringify(bundle));
    fs.writeFileSync(paths[1], JSON.stringify(report));
    const result = spawnSync('python3', ['-', ...paths], { input: binding[1], encoding: 'utf8' });
    const bound = result.status === 0 ? JSON.parse(fs.readFileSync(paths[2], 'utf8')) : null;
    const gate = bound && spawnSync('jq', ['-e', completion[1], paths[2]], { encoding: 'utf8' });
    return { status: result.status, error: result.stderr.trim(), bound, finalizable: gate?.status === 0, gateError: gate?.stderr };
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

// Exercise the next worker's actual processed-report gate as well. Keeping this
// joined to a real roller report prevents copied mock schemas from drifting.
export function checkTapProcessedReport(bundle, report) {
  const source = fs.readFileSync(new URL('../../../scripts/ops-payout-settle.sh', import.meta.url), 'utf8');
  const check = source.match(/validate_tap_processed\(\) \{[\s\S]*?<<'PY'\n([\s\S]*?)\nPY/);
  assert.ok(check, 'production processed-report gate exists');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-processed-report-'));
  try {
    const reportPath = path.join(root, 'report.json'), bundlePath = path.join(root, 'bundle.json');
    fs.writeFileSync(reportPath, JSON.stringify(report));fs.writeFileSync(bundlePath, JSON.stringify(bundle));
    const validation = spawnSync(process.execPath, [new URL('../../../scripts/validate-tap-settlement-checkpoint.mjs', import.meta.url).pathname, reportPath], {encoding:'utf8'});
    if (validation.status !== 0) return {status:validation.status,error:validation.stderr.trim()};
    const result = spawnSync('python3', ['-', reportPath, bundlePath, String(bundle.epoch), bundle.epoch_apply_hash], {input:check[1],encoding:'utf8'});
    return {status:result.status,error:result.stderr.trim()};
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
}
