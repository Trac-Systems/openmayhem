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
