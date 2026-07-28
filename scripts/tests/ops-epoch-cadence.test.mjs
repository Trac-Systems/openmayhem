import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const SCRIPT = path.join(ROOT, 'scripts/ops-epoch-cadence.sh');
const APPLY_HASH = 'a'.repeat(64);

function writeExecutable(target, source) {
  fs.writeFileSync(target, source, { mode: 0o755 });
}

function receiptIndex(epoch, count = 2, revision = count, updatedAt = 'b'.repeat(64)) {
  if (count === 0) return null;
  return {
    type: 'canonical_receipt_epoch_index',
    epoch,
    count,
    page_size: 128,
    page_count: Math.ceil(count / 128),
    revision,
    updated_at: updatedAt,
  };
}

async function harness({
  count = 2,
  pending = false,
  epochSeconds = 60,
} = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-cadence-'));
  const stateDir = path.join(root, 'settlement');
  const source = path.join(root, 'source');
  const scripts = path.join(source, 'scripts');
  const bin = path.join(root, 'bin');
  const rpcState = path.join(root, 'rpc-state.json');
  const rpcServer = path.join(root, 'rpc-server.mjs');
  const rpcPort = path.join(root, 'rpc-port');
  const finalizerLog = path.join(root, 'finalizer.log');
  const mayhemLog = path.join(root, 'mayhem.log');
  const payoutLog = path.join(root, 'payout.log');
  fs.mkdirSync(stateDir, { recursive: true });
  fs.mkdirSync(scripts, { recursive: true });
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(rpcState, `${JSON.stringify({
    apply: {
      updated_epoch: 0,
      pending_epoch: pending ? 1 : null,
      pending_next_page: pending ? 1 : 0,
      last_epoch_seconds: epochSeconds,
      last_apply_hash: pending ? APPLY_HASH : null,
      last_settlement_unix: null,
      last_page: pending ? 0 : null,
    },
    indexes: { 1: receiptIndex(1, count) },
    commits: {},
    seals: {},
    usage: {},
  })}\n`);

  fs.writeFileSync(rpcServer, `
import fs from 'node:fs';
import http from 'node:http';
const [statePath, portPath] = process.argv.slice(2);
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  const key = url.searchParams.get('key');
  const state = JSON.parse(fs.readFileSync(statePath, 'utf8'));
  let value;
  if (key === 'epoch/apply/state') value = state.apply;
  else if (/^epoch\\/commit\\/\\d+$/.test(key ?? '')) {
    value = state.commits?.[key.split('/').at(-1)] ?? null;
  } else if (/^epoch\\/seal\\/\\d+$/.test(key ?? '')) {
    value = state.seals?.[key.split('/').at(-1)] ?? null;
  } else if (/^ev\\/use\\/\\d+$/.test(key ?? '')) {
    value = state.usage?.[key.split('/').at(-1)] ?? null;
  } else {
    const match = /^receipt\\/epoch\\/(\\d+)\\/index$/.exec(key ?? '');
    value = match ? (state.indexes[match[1]] ?? null) : null;
  }
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(JSON.stringify({ key, confirmed: true, value }));
});
server.listen(0, '127.0.0.1', () => {
  fs.writeFileSync(portPath, String(server.address().port));
});
`, { mode: 0o600 });
  const server = spawn(process.execPath, [rpcServer, rpcState, rpcPort], {
    stdio: ['ignore', 'ignore', 'inherit'],
  });
  for (let attempt = 0; attempt < 100 && !fs.existsSync(rpcPort); attempt += 1) {
    await sleep(10);
  }
  assert.equal(fs.existsSync(rpcPort), true, 'fixture RPC did not start');
  const port = fs.readFileSync(rpcPort, 'utf8').trim();

  writeExecutable(path.join(bin, 'date'), `#!/usr/bin/env bash
if [[ "\${1:-}" == "+%s" ]]; then
  printf '%s\\n' '1000'
elif [[ "\${1:-}" == "-u" ]]; then
  printf '%s\\n' '1970-01-01T00:16:40Z'
else
  /bin/date "$@"
fi
`);
  writeExecutable(path.join(bin, 'flock'), '#!/usr/bin/env bash\nexit 0\n');
  writeExecutable(path.join(scripts, 'ops-settle-epoch.sh'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$1" >>"$MOCK_FINALIZER_LOG"
python3 - "$MOCK_RPC_STATE" "$1" <<'PY'
import json, sys
path, epoch = sys.argv[1], int(sys.argv[2])
state = json.load(open(path))
state["apply"].update({
    "updated_epoch": epoch,
    "pending_epoch": None,
    "pending_next_page": 0,
    "last_epoch_seconds": 60,
    "last_apply_hash": "${APPLY_HASH}",
    "last_settlement_unix": 1000,
    "last_page": 1,
})
json.dump(state, open(path, "w"))
PY
`);
  writeExecutable(path.join(scripts, 'ops-payout-settle.sh'), `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$MOCK_PAYOUT_LOG"
exit 91
`);
  writeExecutable(path.join(bin, 'mock-mayhem'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$MOCK_MAYHEM_LOG"
sim=0
for arg in "$@"; do [[ "$arg" == "--sim" ]] && sim=1; done
if (( sim == 0 )); then
  python3 - "$MOCK_RPC_STATE" <<'PY'
import json, sys
path = sys.argv[1]
state = json.load(open(path))
state["apply"].update({
    "updated_epoch": 1,
    "pending_epoch": None,
    "pending_next_page": 0,
    "last_apply_hash": "${APPLY_HASH}",
    "last_settlement_unix": 1000,
    "last_page": 0,
})
json.dump(state, open(path, "w"))
PY
fi
printf '%s\\n' '{"ok":true}'
`);

  const env = {
    PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`,
    HOME: path.join(root, 'home'),
    LANG: 'C',
    MAYHEM_BIN: path.join(bin, 'mock-mayhem'),
    MAYHEM_RPC_URL: `http://127.0.0.1:${port}/v1`,
    MAYHEM_ADMIN_HOME: path.join(root, 'admin-home'),
    MAYHEM_SOURCE_DIR: source,
    MAYHEM_CADENCE_STATE_DIR: stateDir,
    MAYHEM_RECEIPT_QUIET_SECONDS: '1',
    MAYHEM_CADENCE_BOOT_ID: 'test-boot',
    MOCK_RPC_STATE: rpcState,
    MOCK_FINALIZER_LOG: finalizerLog,
    MOCK_MAYHEM_LOG: mayhemLog,
    MOCK_PAYOUT_LOG: payoutLog,
  };
  return {
    root,
    stateDir,
    rpcState,
    finalizerLog,
    mayhemLog,
    payoutLog,
    env,
    close: async () => {
      server.kill('SIGTERM');
      await new Promise((resolve) => server.once('exit', resolve));
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

function runCadence(ctx) {
  return spawnSync('bash', [SCRIPT], {
    cwd: ROOT,
    env: ctx.env,
    encoding: 'utf8',
  });
}

function expireQuietWindow(ctx) {
  const file = path.join(ctx.stateDir, 'cadence.receipt-quiet.json');
  const value = JSON.parse(fs.readFileSync(file, 'utf8'));
  value.observed_at = 0;
  fs.writeFileSync(file, `${JSON.stringify(value)}\n`, { mode: 0o600 });
}

function assertAdvanceStamp(ctx) {
  const file = path.join(ctx.stateDir, 'cadence.last-advance');
  assert.equal(fs.readFileSync(file, 'utf8'), '1000\n');
  assert.equal(fs.statSync(file).mode & 0o777, 0o600);
}

test('nonempty canonical metadata quiets once then invokes only the finalizer', async (t) => {
  const ctx = await harness({ count: 2 });
  t.after(() => ctx.close());
  const first = runCadence(ctx);
  assert.equal(first.status, 0, first.stderr);
  assert.match(first.stdout, /awaiting 1s quiet/);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);

  expireQuietWindow(ctx);
  const second = runCadence(ctx);
  assert.equal(second.status, 0, second.stderr);
  assert.match(second.stdout, /invoking exact-key finalizer/);
  assert.equal(fs.readFileSync(ctx.finalizerLog, 'utf8').trim(), '1');
  assert.equal(fs.existsSync(ctx.payoutLog), false, 'payout maturity must not gate finalization');
  assertAdvanceStamp(ctx);

  const third = runCadence(ctx);
  assert.equal(third.status, 0, third.stderr);
  assert.match(third.stdout, /window has not elapsed/);
  assert.equal(fs.readFileSync(ctx.finalizerLog, 'utf8').trim(), '1');
});

test('null canonical index quiets once then seals the epoch empty', async (t) => {
  const ctx = await harness({ count: 0 });
  t.after(() => ctx.close());
  assert.equal(runCadence(ctx).status, 0);
  expireQuietWindow(ctx);
  const result = runCadence(ctx);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);
  const calls = fs.readFileSync(ctx.mayhemLog, 'utf8').trim().split('\n');
  assert.equal(calls.length, 2);
  assert.match(calls[0], /epoch-seal-empty.*--sim/);
  assert.doesNotMatch(calls[1], /--sim/);
  assert.equal(fs.existsSync(ctx.payoutLog), false);
  assertAdvanceStamp(ctx);
});

test('metadata revision or updated_at change resets the quiet window', async (t) => {
  const ctx = await harness({ count: 2 });
  t.after(() => ctx.close());
  assert.equal(runCadence(ctx).status, 0);
  expireQuietWindow(ctx);
  const state = JSON.parse(fs.readFileSync(ctx.rpcState, 'utf8'));
  state.indexes['1'].revision += 1;
  state.indexes['1'].updated_at = 'c'.repeat(64);
  fs.writeFileSync(ctx.rpcState, `${JSON.stringify(state)}\n`);
  const result = runCadence(ctx);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /awaiting 1s quiet/);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);
});

test('restart resumes a pending targeted page without waiting for payout maturity', async (t) => {
  const ctx = await harness({ count: 2, pending: true });
  t.after(() => ctx.close());
  const state = JSON.parse(fs.readFileSync(ctx.rpcState, 'utf8'));
  Object.assign(state.apply, {
    pending_receipt_index_count: 2,
    pending_receipt_index_revision: 2,
    pending_receipt_index_page_count: 1,
    pending_receipt_index_updated_at: 'b'.repeat(64),
  });
  fs.writeFileSync(ctx.rpcState, `${JSON.stringify(state)}\n`);
  const result = runCadence(ctx);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /resume: pending bounded targeted apply/);
  assert.equal(fs.readFileSync(ctx.finalizerLog, 'utf8').trim(), '1');
  assert.equal(fs.existsSync(ctx.payoutLog), false);
  assertAdvanceStamp(ctx);

  const next = runCadence(ctx);
  assert.equal(next.status, 0, next.stderr);
  assert.match(next.stdout, /window has not elapsed/);
  assert.equal(fs.readFileSync(ctx.finalizerLog, 'utf8').trim(), '1');
});

test('canonical settlement timestamp is required and is the primary cadence clock', async (t) => {
  const ctx = await harness({ count: 2 });
  t.after(() => ctx.close());
  const state = JSON.parse(fs.readFileSync(ctx.rpcState, 'utf8'));
  Object.assign(state.apply, {
    updated_epoch: 1,
    pending_epoch: null,
    last_apply_hash: APPLY_HASH,
    last_settlement_unix: 950,
  });
  state.indexes['2'] = receiptIndex(2, 2);
  fs.writeFileSync(ctx.rpcState, `${JSON.stringify(state)}\n`);
  fs.writeFileSync(path.join(ctx.stateDir, 'cadence.last-advance'), '0\n', {
    mode: 0o600,
  });

  const early = runCadence(ctx);
  assert.equal(early.status, 0, early.stderr);
  assert.match(early.stdout, /window has not elapsed \(canonical settlement 950\)/);
  assert.equal(fs.existsSync(ctx.finalizerLog), false);

  const malformed = JSON.parse(fs.readFileSync(ctx.rpcState, 'utf8'));
  malformed.apply.last_settlement_unix = '950';
  fs.writeFileSync(ctx.rpcState, `${JSON.stringify(malformed)}\n`);
  const rejected = runCadence(ctx);
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stdout, /last_settlement_unix is not a positive canonical timestamp/);
});

test('first v17 cadence step derives time from immutable v16 commit or seal', async (t) => {
  for (const priorKind of ['commit', 'seal']) {
    await t.test(priorKind, async (t) => {
      const ctx = await harness({ count: 2 });
      t.after(() => ctx.close());
      const state = JSON.parse(fs.readFileSync(ctx.rpcState, 'utf8'));
      const commitHash = 'c'.repeat(64);
      const usageRoot = 'd'.repeat(64);
      Object.assign(state.apply, {
        updated_epoch: 1,
        pending_epoch: null,
        last_apply_hash: APPLY_HASH,
        last_receipt_commit_hash: priorKind === 'commit' ? commitHash : null,
      });
      delete state.apply.last_settlement_unix;
      state.indexes['2'] = receiptIndex(2, 2);
      if (priorKind === 'seal') {
        state.seals['1'] = {
          type: 'epoch_empty_seal',
          epoch: 1,
          at: 900,
          seal_hash: APPLY_HASH,
        };
      } else {
        state.commits['1'] = {
          type: 'epoch_commit',
          epoch: 1,
          at: 900,
          commit_hash: commitHash,
          roots: { use: usageRoot },
        };
        state.usage['1'] = {
          type: 'usage_root',
          epoch: 1,
          ts: 900,
          merkle_root: usageRoot,
        };
      }
      fs.writeFileSync(ctx.rpcState, `${JSON.stringify(state)}\n`);

      const result = runCadence(ctx);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /bootstrap: using confirmed epoch 1 settlement timestamp 900/);
      assert.match(result.stdout, /awaiting 1s quiet/);
      assert.equal(fs.existsSync(ctx.finalizerLog), false);
      const after = JSON.parse(fs.readFileSync(ctx.rpcState, 'utf8'));
      assert.equal(Object.hasOwn(after.apply, 'last_settlement_unix'), false);
    });
  }
});
