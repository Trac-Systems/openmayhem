import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const SCRIPT = path.join(ROOT, 'scripts/ops-payout-settle.sh');
const FINALIZER = path.join(ROOT, 'scripts/ops-settle-epoch.sh');
const CADENCE = path.join(ROOT, 'scripts/ops-epoch-cadence.sh');
const INSTALLER = path.join(ROOT, 'scripts/install-mainnet-systemd.sh');
const TAP_ROLLER = path.join(ROOT, 'scripts/ops/run-tap-settlement-roller.sh');
const SERVICE = path.join(ROOT, 'ops/systemd/mayhem-payout-worker.service');
const TIMER = path.join(ROOT, 'ops/systemd/mayhem-payout-worker.timer');
const APPLY_HASH = 'a'.repeat(64);
const FIAT_ROOT = 'b'.repeat(64);
const TNK_ROOT = 'c'.repeat(64);
const TAP_ROOT = `0x${'d'.repeat(64)}`;
const TAP_TX = `0x${'e'.repeat(64)}`;

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}

function writeJson(target, value) {
  const raw = `${JSON.stringify(value, null, 2)}\n`;
  fs.writeFileSync(target, raw);
  return raw;
}

function writeExecutable(target, source) {
  fs.writeFileSync(target, source, { mode: 0o755 });
}

function harness({ bundle = true, emptySeal = !bundle } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-payout-worker-'));
  const state = path.join(root, 'settlement');
  const spool = path.join(state, 'tap');
  const bin = path.join(root, 'bin');
  const log = path.join(root, 'mayhem.log');
  const applyState = path.join(root, 'apply-state.json');
  const emptySealDefault = emptySeal ? '1' : '0';
  fs.mkdirSync(bin, { recursive: true });
  writeJson(applyState, {
    value: {
      updated_epoch: 7,
      pending_epoch: null,
      last_apply_hash: APPLY_HASH.toUpperCase(),
    },
  });
  const nowFile = path.join(root, 'now');
  fs.writeFileSync(nowFile, '1000\n');

  writeExecutable(path.join(bin, 'curl'), `#!/usr/bin/env bash
if [[ "$*" == *"prefix=payout/liability/tnk/"* && "\${MOCK_TNK_OUTSTANDING:-0}" == "1" ]]; then
  printf '%s\\n' '{"values":[{"key":"payout/liability/tnk/provider/revision","value":{"total_au":"10","paid_cum_au":"0"}}]}'
elif [[ "$*" == *"key=fee/tnk/cum"* ]]; then
  printf '%s\\n' '{"value":{"cum_au":"0","swept_cum_au":"0"}}'
elif [[ "$*" == *"key=ev/dep/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"deposit_root","epoch":7,"merkle_root":"${'1'.repeat(64)}","count":0,"au_total":"0"}}'
elif [[ "$*" == *"key=ev/use/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"usage_root","epoch":7,"merkle_root":"${'2'.repeat(64)}","sessions":3,"au_total":"30","providers":3}}'
elif [[ "$*" == *"key=ev/earn/7"* ]]; then
  if [[ "\${MOCK_STALE_ROOT:-0}" == "1" ]]; then
    printf '%s\\n' '{"value":{"type":"earn_root","epoch":7,"merkle_root":"${'9'.repeat(64)}","provider_count":3,"au_cum_total":"20"}}'
  else
    printf '%s\\n' '{"value":{"type":"earn_root","epoch":7,"merkle_root":"${'3'.repeat(64)}","provider_count":3,"au_cum_total":"20"}}'
  fi
elif [[ "$*" == *"key=ev/fee/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"fee_root","epoch":7,"merkle_root":"${'4'.repeat(64)}","au_fee_epoch":"5","au_fee_cum":"5","au_burn_epoch":"1","au_burn_cum":"1"}}'
elif [[ "$*" == *"key=ev/price/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"price_root","epoch":7,"merkle_root":"${'5'.repeat(64)}","price_count":0}}'
elif [[ "$*" == *"key=epoch/seal/7"* ]]; then
  if [[ "\${MOCK_EMPTY_SEAL:-${emptySealDefault}}" == "1" ]]; then
    printf '%s\\n' '{"value":{"type":"epoch_empty_seal","epoch":7,"seal_hash":"${APPLY_HASH}","totals":{"debited_au":"0","earned_au":"0","fee_au":"0","burn_au":"0"}}}'
  else
    printf '%s\\n' '{"value":null}'
  fi
else
  cat "$MOCK_APPLY_STATE"
fi
`);
  writeExecutable(path.join(bin, 'date'), `#!/usr/bin/env bash
if [[ "\${1:-}" == "+%s" ]]; then
  cat "$MOCK_NOW_FILE"
else
  /bin/date "$@"
fi
`);
  writeExecutable(path.join(bin, 'mock-mayhem'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$MOCK_MAYHEM_LOG"
rail="\${2:-}"
submit=0
for arg in "$@"; do
  [[ "$arg" == "--submit-transfer" ]] && submit=1
done
if [[ "$rail" == "fiat-settlement" ]]; then
  fiat_plan='{"ok":true,"epoch":7,"submitted":false,"already_settled":null,"nothing_to_settle":false,"settlement":{"op":"settle_targeted_fiat","rail":"fiat","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${FIAT_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"acct_provider","currency":"eur","amount_minor":"100","au":"10000000000000000"}],"stripe_transfers":[]},"skipped_providers":[]}'
  fiat_empty='{"ok":true,"epoch":7,"submitted":false,"already_settled":null,"nothing_to_settle":true,"settlement":{"op":"settle_targeted_fiat","rail":"fiat","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${FIAT_ROOT}","outputs":[],"stripe_transfers":[]},"skipped_providers":[]}'
  fiat_final='{"ok":true,"epoch":7,"submitted":true,"already_settled":null,"nothing_to_settle":false,"settlement":{"op":"settle_targeted_fiat","rail":"fiat","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${FIAT_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"acct_provider","currency":"eur","amount_minor":"100","au":"10000000000000000"}],"stripe_transfers":[{"schema_version":1,"kind":"stripe_transfer","ref":"tr_1","destination":"acct_provider","currency":"eur","amount_minor":"100","transfer_group":"epoch-7"}]},"settlement_state":{"op":"settle_targeted_fiat","rail":"fiat","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${FIAT_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"acct_provider","currency":"eur","amount_minor":"100","au":"10000000000000000"}],"stripe_transfers":[{"schema_version":1,"kind":"stripe_transfer","ref":"tr_1","destination":"acct_provider","currency":"eur","amount_minor":"100","transfer_group":"epoch-7"}]},"stripe_transfers":[{"output_index":0,"transfer":{"id":"tr_1","verified":true}}],"reconciliation":{"all_provider_transfers_verified":true},"skipped_providers":[]}'
  if [[ "\${MOCK_FIAT_MODE:-success}" == "blocking" ]]; then
    printf '%s\\n' "\${fiat_plan/\\\"skipped_providers\\\":[]/\\\"skipped_providers\\\":[{\\\"blocking\\\":true}]}"
  elif [[ "\${MOCK_FIAT_MODE:-success}" == "no_work" ]]; then
    printf '%s\\n' "$fiat_empty"
  elif [[ "\${MOCK_FIAT_MODE:-success}" == "stale_epoch" ]]; then
    printf '%s\\n' "\${fiat_plan/\\\"epoch\\\":7,\\\"epoch_apply_hash\\\"/\\\"epoch\\\":6,\\\"epoch_apply_hash\\\"}"
  elif (( submit == 1 )); then
    printf '%s\\n' "$fiat_final"
  else
    printf '%s\\n' "$fiat_plan"
  fi
elif [[ "$rail" == "tnk-settlement" ]]; then
  tnk_plan='{"ok":true,"epoch":7,"submitted":false,"already_settled":null,"settlement":{"op":"settle_targeted_tnk","rail":"tnk","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${TNK_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"trac1provider","au":"10","tnk_e18":"10"}],"msb_transfers":[]},"skipped_providers":[],"msb_outputs":[{"to":"trac1provider","amount":"0.000000000000000010"}]}'
  tnk_final='{"ok":true,"epoch":7,"submitted":true,"already_settled":null,"settlement":{"op":"settle_targeted_tnk","rail":"tnk","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${TNK_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"trac1provider","au":"10","tnk_e18":"10"}],"msb_transfers":[{"schema_version":1,"network":"mainnet","tx_hash":"${'f'.repeat(64)}","confirmed_length":9,"observed_signed_length":10,"from":"trac1treasury","to":"trac1provider","amount_e18":"10"}]},"settlement_state":{"op":"settle_targeted_tnk","rail":"tnk","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${TNK_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"trac1provider","au":"10","tnk_e18":"10"}],"msb_transfers":[{"schema_version":1,"network":"mainnet","tx_hash":"${'f'.repeat(64)}","confirmed_length":9,"observed_signed_length":10,"from":"trac1treasury","to":"trac1provider","amount_e18":"10"}]},"msb_outputs":[{"to":"trac1provider","amount":"0.000000000000000010"}],"msb_transfers":[{"output_index":0,"operation_id":"op","transfer":{"tx_hash":"${'f'.repeat(64)}","confirmed_length":9,"observed_signed_length":10,"from":"trac1treasury","to":"trac1provider"}}],"skipped_providers":[]}'
  if [[ "\${MOCK_TNK_MODE:-success}" == "no_work" ]]; then
    printf '%s\\n' 'TNK settlement has no positive provider or operator fee outputs; nothing to broadcast' >&2
    exit 1
  elif (( submit == 1 )); then
    printf '%s\\n' "$tnk_final"
  else
    printf '%s\\n' "$tnk_plan"
  fi
else
  printf '%s\\n' "unexpected mock mayhem command: $*" >&2
  exit 2
fi
`);

  if (bundle) {
    const epochDir = path.join(state, 'epochs/epoch-7');
    fs.mkdirSync(epochDir, { recursive: true });
    const entry = (rail, id) => ({
      rail,
      receipt: {
        body: {
          rail,
          session_id: `${rail}-${id}`,
        },
      },
    });
    const receipts = [
      entry('fiat', 1),
      entry('tap', 2),
      entry('tnk', 3),
    ];
    const gatewayReceiptsRaw = writeJson(
      path.join(epochDir, 'gateway-receipts.json'),
      receipts
    );
    const bundleRaw = writeJson(path.join(epochDir, 'epoch-bundle.json'), {
      epoch: 7,
      params: { fee_bps: 1500 },
      receipts,
    });
    const recomputed = {
      epoch: 7,
      roots: Object.fromEntries(
        ['dep', 'use', 'earn', 'fee', 'price'].map(
          (key, index) => [key, String(index + 1).repeat(64)]
        )
      ),
      totals: {
        dep_count: 0,
        dep_au: '0',
        use_count: 3,
        use_au: '30',
        provider_count: 3,
        earn_au: '20',
        fee_au: '5',
        fee_cum_au: '5',
        burn_au: '1',
        burn_cum_au: '1',
        price_count: 0,
      },
    };
    const recomputedRaw = writeJson(path.join(epochDir, 'epoch-recomputed.json'), recomputed);
    writeJson(path.join(epochDir, 'epoch-artifact.json'), {
      schema_version: 1,
      type: 'retained_epoch_artifact',
      rail: 'all',
      rails: ['fiat', 'tap', 'tnk'],
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(bundleRaw),
      recomputed_sha256: sha256(recomputedRaw),
      gateway_receipts_sha256: sha256(gatewayReceiptsRaw),
      roots: recomputed.roots,
      totals: recomputed.totals,
    });
  }

  const env = {
    PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`,
    HOME: path.join(root, 'home'),
    LANG: 'C',
    MAYHEM_BIN: path.join(bin, 'mock-mayhem'),
    MAYHEM_RPC_URL: 'http://mock.invalid/v1',
    MAYHEM_ADMIN_HOME: path.join(root, 'admin-home'),
    MAYHEM_ADMIN_STORE: 'test-admin',
    MAYHEM_CADENCE_STATE_DIR: state,
    MAYHEM_TAP_SETTLEMENT_SPOOL: spool,
    MAYHEM_PAYOUT_LOCK_HELD: '1',
    MAYHEM_PAYOUT_TEST_MODE: '1',
    MAYHEM_PAYOUT_TEST_ROOT: root,
    MOCK_APPLY_STATE: applyState,
    MOCK_MAYHEM_LOG: log,
    MOCK_NOW_FILE: nowFile,
  };
  return { root, state, spool, log, nowFile, env };
}

function runWorker(ctx, extraEnv = {}) {
  return spawnSync('bash', [SCRIPT], {
    cwd: ROOT,
    env: { ...ctx.env, ...extraEnv },
    encoding: 'utf8',
  });
}

function logLines(ctx) {
  if (!fs.existsSync(ctx.log)) return [];
  return fs.readFileSync(ctx.log, 'utf8').trim().split('\n').filter(Boolean);
}

function rebindArtifact(ctx) {
  const epochDir = path.join(ctx.state, 'epochs/epoch-7');
  const bundleRaw = fs.readFileSync(path.join(epochDir, 'epoch-bundle.json'));
  const recomputedRaw = fs.readFileSync(path.join(epochDir, 'epoch-recomputed.json'));
  const bundle = JSON.parse(bundleRaw);
  writeJson(path.join(epochDir, 'gateway-receipts.json'), bundle.receipts);
  const gatewayReceiptsRaw = fs.readFileSync(path.join(epochDir, 'gateway-receipts.json'));
  const recomputed = JSON.parse(recomputedRaw);
  writeJson(path.join(epochDir, 'epoch-artifact.json'), {
    schema_version: 1,
    type: 'retained_epoch_artifact',
    rail: 'all',
    rails: ['fiat', 'tap', 'tnk'],
    epoch: 7,
    epoch_apply_hash: APPLY_HASH,
    bundle_sha256: sha256(bundleRaw),
    recomputed_sha256: sha256(recomputedRaw),
    gateway_receipts_sha256: sha256(gatewayReceiptsRaw),
    roots: recomputed.roots,
    totals: recomputed.totals,
  });
}

function writeTapReport(ctx, bundleName, overrides = {}) {
  const processed = path.join(ctx.spool, 'processed');
  const bundleRaw = fs.readFileSync(path.join(processed, bundleName));
  writeJson(
    path.join(processed, `epoch-7-${APPLY_HASH}.settlement.json`),
    {
      rail: 'tap',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(bundleRaw),
      root: TAP_ROOT,
      blocked: false,
      root_confirmed: true,
      posted: true,
      proposal_tx: TAP_TX,
      operator_fee: {
        completed: true,
        predicted_claimable_wei: '0',
        remaining_claimable_wei: '0',
      },
      burn: {
        completed: true,
        predicted_claimable_wei: '0',
        remaining_claimable_wei: '0',
      },
      ...overrides,
    }
  );
}

test('payout worker isolates TAP spool work and replays all rails idempotently', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const first = runWorker(ctx);
  assert.equal(first.status, 0, first.stderr);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  const queuedSummary = JSON.parse(fs.readFileSync(path.join(workDir, 'summary.json'), 'utf8'));
  assert.equal(queuedSummary.complete, false);
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(queuedSummary.rails).map(([rail, value]) => [rail, value.complete])
    ),
    { fiat: true, tap: false, tnk: true }
  );

  const ready = fs.readdirSync(path.join(ctx.spool, 'ready'));
  assert.deepEqual(ready, [`epoch-7-${APPLY_HASH}.receipts.json`]);
  const tapBundle = JSON.parse(
    fs.readFileSync(path.join(ctx.spool, 'ready', ready[0]), 'utf8')
  );
  assert.equal(tapBundle.receipts.length, 1);
  assert.equal(tapBundle.receipts[0].rail, 'tap');
  assert.equal(tapBundle.receipts[0].receipt.body.rail, 'tap');
  assert.equal(tapBundle.receipts[0].receipt_epoch, 7);
  assert.equal(tapBundle.rail, 'tap');
  assert.equal(tapBundle.epoch_apply_hash, APPLY_HASH);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.produced')), true);

  const firstLog = logLines(ctx);
  assert.equal(firstLog.length, 4);
  assert.equal(firstLog.filter((line) => line.startsWith('admin fiat-settlement')).length, 2);
  assert.equal(firstLog.filter((line) => line.startsWith('admin tnk-settlement')).length, 2);

  const working = path.join(ctx.spool, 'working');
  fs.renameSync(
    path.join(ctx.spool, 'ready', ready[0]),
    path.join(working, ready[0])
  );
  const pendingReplay = runWorker(ctx);
  assert.equal(pendingReplay.status, 0, pendingReplay.stderr);
  assert.deepEqual(logLines(ctx), firstLog);
  assert.equal(fs.readFileSync(path.join(workDir, 'tap.attempts'), 'utf8').trim(), '1');
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
  assert.deepEqual(fs.readdirSync(working), ready);

  const processed = path.join(ctx.spool, 'processed');
  fs.mkdirSync(processed, { recursive: true });
  fs.renameSync(
    path.join(working, ready[0]),
    path.join(processed, ready[0])
  );
  const processedBundle = fs.readFileSync(path.join(processed, ready[0]));
  writeJson(
    path.join(processed, `epoch-7-${APPLY_HASH}.settlement.json`),
    {
      rail: 'tap',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(processedBundle),
      root: TAP_ROOT,
      blocked: false,
      root_confirmed: true,
      posted: true,
      proposal_tx: TAP_TX,
      operator_fee: {
        completed: true,
        predicted_claimable_wei: '0',
        remaining_claimable_wei: '0',
      },
      burn: {
        completed: true,
        predicted_claimable_wei: '0',
        remaining_claimable_wei: '0',
      },
    }
  );
  const second = runWorker(ctx);
  assert.equal(second.status, 0, second.stderr);
  assert.deepEqual(logLines(ctx), firstLog);
  const settledSummary = JSON.parse(fs.readFileSync(path.join(workDir, 'summary.json'), 'utf8'));
  assert.equal(settledSummary.complete, true);
  assert.equal(settledSummary.rails.tap.result.status, 'settled');
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
});

test('TAP queue publication survives a crash without a duplicate item or complete marker', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const crashed = runWorker(ctx, { MAYHEM_PAYOUT_TEST_CRASH_AFTER_TAP_QUEUE: '1' });
  assert.notEqual(crashed.status, 0);
  assert.match(crashed.stderr, /simulated crash after atomic queue publication/);

  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), [name]);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.produced')), false);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);

  const replay = runWorker(ctx);
  assert.equal(replay.status, 0, replay.stderr);
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), [name]);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.produced')), true);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
  assert.equal(fs.readFileSync(path.join(workDir, 'tap.attempts'), 'utf8').trim(), '1');
});

test('duplicate TAP lifecycle entries are rejected without burning an attempt', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  assert.equal(runWorker(ctx).status, 0);

  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.copyFileSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'working', name)
  );
  const replay = runWorker(ctx);
  assert.notEqual(replay.status, 0);
  assert.match(replay.stderr, /duplicate spool item exists in more than one lifecycle state/);
  assert.equal(fs.readFileSync(path.join(workDir, 'tap.attempts'), 'utf8').trim(), '1');
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
});

test('stale fiat evidence is rejected before a completion marker is written', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'stale_epoch',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /plan is not bound to the current rail\/epoch\/apply hash/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), false);
});

test('cross-epoch TAP processed evidence is rejected', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  assert.equal(runWorker(ctx).status, 0);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.renameSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'processed', name)
  );
  writeTapReport(ctx, name, { epoch: 8 });

  const replay = runWorker(ctx);
  assert.notEqual(replay.status, 0);
  assert.match(replay.stderr, /processed spool item lacks exact settlement evidence/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
});

test('outer-only and cross-rail receipt classification are rejected', async (t) => {
  await t.test('outer rail cannot substitute for a signed body rail', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const bundlePath = path.join(ctx.state, 'epochs/epoch-7/epoch-bundle.json');
    const bundle = JSON.parse(fs.readFileSync(bundlePath, 'utf8'));
    delete bundle.receipts[1].receipt.body.rail;
    writeJson(bundlePath, bundle);
    rebindArtifact(ctx);

    const result = runWorker(ctx);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /failed to derive a rail-isolated spool bundle/);
    assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
  });

  await t.test('outer rail must exactly match the signed body rail', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const bundlePath = path.join(ctx.state, 'epochs/epoch-7/epoch-bundle.json');
    const bundle = JSON.parse(fs.readFileSync(bundlePath, 'utf8'));
    bundle.receipts[1].rail = 'fiat';
    writeJson(bundlePath, bundle);
    rebindArtifact(ctx);

    const result = runWorker(ctx);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /failed to derive a rail-isolated spool bundle/);
    assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
  });
});

test('stale retained epoch artifact and inherited live credentials are refused', async (t) => {
  await t.test('stale artifact apply hash', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const artifactPath = path.join(ctx.state, 'epochs/epoch-7/epoch-artifact.json');
    const artifact = JSON.parse(fs.readFileSync(artifactPath, 'utf8'));
    artifact.epoch_apply_hash = '9'.repeat(64);
    writeJson(artifactPath, artifact);

    const result = runWorker(ctx);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /does not match the canonical epoch\/apply hash/);
    assert.deepEqual(logLines(ctx), []);
  });

  await t.test('stale canonical root evidence', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const result = runWorker(ctx, { MOCK_STALE_ROOT: '1' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /canonical ev\/earn root does not match retained epoch evidence/);
    assert.deepEqual(logLines(ctx), []);
  });

  await t.test('test mode credential isolation', () => {
    const ctx = harness({ bundle: false });
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const result = runWorker(ctx, { MAYHEM_STRIPE_SECRET_KEY: 'sk_live_must_not_leak' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /refuses inherited credential MAYHEM_STRIPE_SECRET_KEY/);
    assert.deepEqual(logLines(ctx), []);
  });
});

test('payout worker records canonical no-work outcomes without requiring live keys', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.equal(result.status, 0, result.stderr);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  for (const rail of ['fiat', 'tap', 'tnk']) {
    const marker = JSON.parse(fs.readFileSync(path.join(workDir, `${rail}.complete`), 'utf8'));
    assert.equal(marker.status, 'no_work');
  }
  assert.equal(logLines(ctx).length, 2);
});

test('bounded payout attempts reopen automatically after backoff and transient recovery', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const extraEnv = {
    MOCK_FIAT_MODE: 'blocking',
    MOCK_TNK_MODE: 'no_work',
    MAYHEM_PAYOUT_MAX_ATTEMPTS: '2',
    MAYHEM_PAYOUT_RETRY_BACKOFF_SECONDS: '300',
  };

  assert.notEqual(runWorker(ctx, extraEnv).status, 0);
  assert.notEqual(runWorker(ctx, extraEnv).status, 0);
  const backingOff = runWorker(ctx, extraEnv);
  assert.equal(backingOff.status, 0, backingOff.stderr);
  assert.match(backingOff.stderr, /retry window reopens at 1300/);
  assert.equal(
    logLines(ctx).filter((line) => line.startsWith('admin fiat-settlement')).length,
    2
  );
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), false);
  assert.equal(fs.readFileSync(path.join(workDir, 'fiat.attempts'), 'utf8').trim(), '2');

  fs.writeFileSync(ctx.nowFile, '1300\n');
  const recovered = runWorker(ctx, {
    ...extraEnv,
    MOCK_FIAT_MODE: 'no_work',
  });
  assert.equal(recovered.status, 0, recovered.stderr);
  assert.match(recovered.stderr, /reopening fiat payout attempts/);
  assert.equal(fs.readFileSync(path.join(workDir, 'fiat.attempts'), 'utf8').trim(), '1');
  assert.equal(fs.existsSync(path.join(workDir, 'complete')), true);
});

test('TNK no-output is not accepted while canonical liabilities remain', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'no_work',
    MOCK_TNK_OUTSTANDING: '1',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /canonical TNK liabilities remain held or blocked/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tnk.complete')), false);
});

test('missing TAP bundle fails unless the exact apply hash is an empty-epoch seal', (t) => {
  const ctx = harness({ bundle: false, emptySeal: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /non-empty epoch is missing its retained receipt bundle/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
});

test('systemd and finalizer wiring preserve the automatic payout handoff', () => {
  const finalizer = fs.readFileSync(FINALIZER, 'utf8');
  const cadence = fs.readFileSync(CADENCE, 'utf8');
  const installer = fs.readFileSync(INSTALLER, 'utf8');
  const tapRoller = fs.readFileSync(TAP_ROLLER, 'utf8');
  const service = fs.readFileSync(SERVICE, 'utf8');
  const timer = fs.readFileSync(TIMER, 'utf8');

  assert.match(finalizer, /epochs\/epoch-\$epoch/);
  assert.match(finalizer, /ops-payout-settle\.sh/);
  assert.match(finalizer, /bind_epoch_artifact/);
  assert.match(finalizer, /outer_rail != rail/);
  assert.match(finalizer, /payouts remain incomplete; refusing to finalize/);
  assert.doesNotMatch(finalizer, /admin fiat-settlement/);
  assert.match(cadence, /ops-payout-settle\.sh/);
  assert.match(cadence, /ops-settle-epoch\.sh" "\$target"/);
  assert.doesNotMatch(cadence, /manual settlement required|receipts export ->/);
  assert.match(cadence, /current epoch \$updated_epoch payouts remain incomplete/);
  assert.match(cadence, /payout\.lock/);
  assert.match(service, /ExecStart=\/opt\/mayhem\/source\/scripts\/ops-payout-settle\.sh/);
  assert.match(service, /mayhem-tap-settlement\.service/);
  assert.match(timer, /OnUnitActiveSec=1min/);
  assert.match(installer, /systemctl enable --now mayhem-payout-worker\.timer/);
  assert.match(installer, /require_private_key MAYHEM_TAP_ROLLER_PRIVATE_KEY/);
  assert.match(installer, /require_file_env MAYHEM_TNK_TREASURY_KEYPAIR_PATH/);
  assert.match(tapRoller, /find "\$working"/);
  assert.match(tapRoller, /timeout "\$attempt_timeout"/);
});
