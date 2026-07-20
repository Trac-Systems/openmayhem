import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const SCRIPT = path.join(ROOT, 'scripts/ops-epoch-cadence.sh');
const APPLY_HASH = 'a'.repeat(64);

function writeExecutable(target, source) {
  fs.writeFileSync(target, source, { mode: 0o755 });
}

function writeApplyState(target, updatedEpoch, epochSeconds = 60) {
  fs.writeFileSync(target, `${JSON.stringify({
    value: {
      updated_epoch: updatedEpoch,
      pending_epoch: null,
      last_epoch_seconds: epochSeconds,
      last_apply_hash: updatedEpoch === 0 ? null : APPLY_HASH,
    },
  })}\n`);
}

function harness({ retainedReceipts = 1, epochSeconds = 60 } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-cadence-'));
  const state = path.join(root, 'settlement');
  const source = path.join(root, 'source');
  const scripts = path.join(source, 'scripts');
  const bin = path.join(root, 'bin');
  const applyState = path.join(root, 'apply-state.json');
  const finalizerLog = path.join(root, 'finalizer.log');
  const mayhemLog = path.join(root, 'mayhem.log');
  fs.mkdirSync(state, { recursive: true });
  fs.mkdirSync(scripts, { recursive: true });
  fs.mkdirSync(bin, { recursive: true });
  writeApplyState(applyState, 0, epochSeconds);

  writeExecutable(path.join(bin, 'curl'), `#!/usr/bin/env bash
if [[ "$*" == *"/mayhem/status"* ]]; then
  printf '%s\\n' '{"sessions_active":0}'
elif [[ "$*" == *"/mayhem/receipts"* ]]; then
  if [[ "$MOCK_RETAINED_RECEIPTS" == "1" ]]; then
    printf '%s\\n' '{"object":"list","data":[{"rail":"tap","receipt":{"body":{"rail":"tap"}}}]}'
  else
    printf '%s\\n' '{"object":"list","data":[]}'
  fi
else
  cat "$MOCK_APPLY_STATE"
fi
`);
  writeExecutable(path.join(bin, 'date'), `#!/usr/bin/env bash
if [[ "\${1:-}" == "+%s" ]]; then
  printf '%s\\n' '1000'
elif [[ "\${1:-}" == "-u" ]]; then
  printf '%s\\n' '1970-01-01T00:16:40Z'
else
  /bin/date "$@"
fi
`);
  writeExecutable(path.join(bin, 'flock'), `#!/usr/bin/env bash
exit 0
`);
  writeExecutable(path.join(scripts, 'ops-settle-epoch.sh'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$1" >>"$MOCK_FINALIZER_LOG"
cat >"$MOCK_APPLY_STATE" <<JSON
{"value":{"updated_epoch":$1,"pending_epoch":null,"last_epoch_seconds":60,"last_apply_hash":"${APPLY_HASH}"}}
JSON
`);
  writeExecutable(path.join(scripts, 'ops-payout-settle.sh'), `#!/usr/bin/env bash
if [[ "\${MOCK_ALLOW_PAYOUT:-0}" == "1" ]]; then
  mkdir -p "$MAYHEM_CADENCE_STATE_DIR/payout/epoch-$1-${APPLY_HASH}"
  printf '%s\\n' '{"complete":true}' >"$MAYHEM_CADENCE_STATE_DIR/payout/epoch-$1-${APPLY_HASH}/complete"
  exit 0
fi
printf '%s\\n' "unexpected payout worker call: $*" >&2
exit 91
`);
  writeExecutable(path.join(bin, 'mock-mayhem'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$MOCK_MAYHEM_LOG"
sim=0
for arg in "$@"; do
  [[ "$arg" == "--sim" ]] && sim=1
done
if (( sim == 0 )); then
  cat >"$MOCK_APPLY_STATE" <<JSON
{"value":{"updated_epoch":1,"pending_epoch":null,"last_epoch_seconds":60,"last_apply_hash":"${APPLY_HASH}"}}
JSON
fi
printf '%s\\n' '{"ok":true}'
`);

  const env = {
    PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`,
    HOME: path.join(root, 'home'),
    LANG: 'C',
    MAYHEM_BIN: path.join(bin, 'mock-mayhem'),
    MAYHEM_RPC_URL: 'http://mock.invalid/v1',
    MAYHEM_GATEWAY_URL: 'http://gateway.mock.invalid',
    MAYHEM_ADMIN_HOME: path.join(root, 'admin-home'),
    MAYHEM_SOURCE_DIR: source,
    MAYHEM_CADENCE_STATE_DIR: state,
    MOCK_APPLY_STATE: applyState,
    MOCK_FINALIZER_LOG: finalizerLog,
    MOCK_MAYHEM_LOG: mayhemLog,
    MOCK_RETAINED_RECEIPTS: String(retainedReceipts),
    MOCK_ALLOW_PAYOUT: '0',
  };
  return { root, state, finalizerLog, mayhemLog, env };
}

function runCadence(ctx) {
  return spawnSync('bash', [SCRIPT], {
    cwd: ROOT,
    env: ctx.env,
    encoding: 'utf8',
  });
}

test('retained receipts invoke only the canonical gated finalizer', (t) => {
  const ctx = harness({ retainedReceipts: 1 });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runCadence(ctx);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /invoking canonical finalizer for epoch 1/);
  assert.equal(fs.readFileSync(ctx.finalizerLog, 'utf8').trim(), '1');
  assert.equal(fs.existsSync(ctx.mayhemLog), false);
});

test('an empty epoch still uses simulate-first canonical sealing', (t) => {
  const ctx = harness({ retainedReceipts: 0 });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runCadence(ctx);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);
  const calls = fs.readFileSync(ctx.mayhemLog, 'utf8').trim().split('\n');
  assert.equal(calls.length, 2);
  assert.match(calls[0], /admin epoch-seal-empty/);
  assert.match(calls[0], /--sim/);
  assert.doesNotMatch(calls[1], /--sim/);
});

test('invalid arithmetic input aborts before any finalization command', (t) => {
  const ctx = harness({ retainedReceipts: 0, epochSeconds: '1+1' });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runCadence(ctx);
  assert.notEqual(result.status, 0);
  assert.match(result.stdout, /last_epoch_seconds is not a positive canonical integer/);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);
  assert.equal(fs.existsSync(ctx.mayhemLog), false);
});

test('receipts already bound to the prior epoch cannot contaminate the next epoch', (t) => {
  const ctx = harness({ retainedReceipts: 1 });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  writeApplyState(path.join(ctx.root, 'apply-state.json'), 1, 60);
  const priorDir = path.join(ctx.state, 'epochs/epoch-1');
  fs.mkdirSync(priorDir, { recursive: true });
  fs.writeFileSync(
    path.join(priorDir, 'gateway-receipts.json'),
    '[{"rail":"tap","receipt":{"body":{"rail":"tap"}}}]\n'
  );
  ctx.env.MOCK_ALLOW_PAYOUT = '1';

  const result = runCadence(ctx);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /still retains 1 receipt\(s\) already bound to epoch 1/);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);
  assert.equal(fs.existsSync(ctx.mayhemLog), false);
});
