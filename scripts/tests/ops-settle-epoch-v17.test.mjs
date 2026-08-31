import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

import { opaqueHash } from '../../intercom/scripts/recompute-epoch-roots.mjs';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const SCRIPT = path.join(ROOT, 'scripts/ops-settle-epoch.sh');
const COMMIT_HASH = '4'.repeat(64);
const APPLY_HASH = '5'.repeat(64);
const hex = (value) => value.toString(16).padStart(64, '0').slice(-64);

function writeExecutable(target, source) {
  fs.writeFileSync(target, source, { mode: 0o755 });
}

async function receiptHead(ordinal, {
  settlementEpoch = 1,
  billingEpoch = settlementEpoch,
} = {}) {
  const body = {
    schema_version: 11,
    session_id: hex(1_000 + ordinal),
    billing_id: hex(ordinal),
    billing_attempt: 0,
    billing_epoch: billingEpoch,
    reservation_id: hex(2_000 + ordinal),
    payout_revision: 'a'.repeat(64),
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    seq: 1,
    final: true,
    rail: ordinal === 2 ? 'tap' : 'fiat',
    user: hex(3_000 + ordinal),
    provider: ordinal === 2 ? 'b'.repeat(64) : 'c'.repeat(64),
    enclave_id: 'd'.repeat(64),
    model_id: 'fixture/model',
    price_ver: 1,
    locked_rate_map: [{ unit: 'output_token', per_unit_au: '1', granularity: 1 }],
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 1_024,
    ctx_bracket: 'le32k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { output_token: 1 },
    au_owed_cum: '1',
    prompt_hash: 'e'.repeat(64),
    ts: 3_600,
  };
  const receipt = {
    body,
    enclave_sig: '1'.repeat(128),
    enclave_pubkey: '2'.repeat(64),
    user_sig: '3'.repeat(128),
  };
  return {
    epoch: settlementEpoch,
    billing_epoch: body.billing_epoch,
    billing_id: body.billing_id,
    billing_attempt: 0,
    reservation_id: body.reservation_id,
    payout_revision: body.payout_revision,
    receipt_hash: await opaqueHash('mayhem-canonical-receipt-v1', receipt),
    incremental_au: '1',
    receipt,
  };
}

async function harness({
  failPageOnce = null,
  driftAtIndexRead = null,
  bareIndexOnly = false,
  staleSnapshotAndCommit = false,
  settlementEpoch = 1,
  billingEpoch = settlementEpoch,
} = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-finalizer-'));
  const bin = path.join(root, 'bin');
  const home = path.join(root, 'admin-home');
  const stateDir = path.join(root, 'settlement');
  const rpcStatePath = path.join(root, 'rpc-state.json');
  const rpcServerPath = path.join(root, 'rpc-server.mjs');
  const rpcPortPath = path.join(root, 'rpc-port');
  const mayhemLog = path.join(root, 'mayhem.log');
  fs.mkdirSync(bin, { recursive: true });
  fs.mkdirSync(home, { recursive: true });
  fs.mkdirSync(stateDir, { recursive: true });
  fs.symlinkSync(process.execPath, path.join(bin, 'node'));

  const heads = [
    await receiptHead(3, { settlementEpoch, billingEpoch }),
    await receiptHead(1, { settlementEpoch, billingEpoch }),
    await receiptHead(2, { settlementEpoch, billingEpoch }),
  ];
  const index = {
    type: 'canonical_receipt_epoch_index',
    epoch: settlementEpoch,
    count: heads.length,
    page_size: 128,
    page_count: 1,
    revision: heads.length,
    updated_at: 'f'.repeat(64),
  };
  const records = {
    [bareIndexOnly
      ? `receipt/epoch/${settlementEpoch}`
      : `receipt/epoch/${settlementEpoch}/index`]: index,
    [`receipt/epoch/${settlementEpoch}/page/0`]: {
      type: 'canonical_receipt_epoch_page',
      epoch: settlementEpoch,
      page: 0,
      identities: heads.map((head) => ({
        billing_id: head.billing_id,
        billing_attempt: head.billing_attempt,
      })),
    },
    'params/fee_bps': {
      key: 'fee_bps',
      current: { value: 1_500, effective_at: 0 },
      pending: null,
    },
    'params/max_apply_batch': {
      key: 'max_apply_batch',
      current: { value: 4, effective_at: 0 },
      pending: null,
    },
    'params/max_market_usage_entries': {
      key: 'max_market_usage_entries',
      current: { value: 10, effective_at: 0 },
      pending: null,
    },
  };
  for (const head of heads) {
    records[`receipt/head/${head.billing_id}/${head.billing_attempt}`] = head;
  }
  if (staleSnapshotAndCommit) {
    const staleRunDir = path.join(stateDir, `epochs/epoch-${settlementEpoch}`);
    fs.mkdirSync(staleRunDir, { recursive: true });
    fs.writeFileSync(path.join(staleRunDir, 'canonical-receipts.json'), JSON.stringify({
      schema_version: 1,
      type: 'canonical_epoch_receipt_snapshot',
      settlement_epoch: settlementEpoch,
      metadata: { ...index, count: 1, revision: 1 },
      identities: [],
      heads: [],
    }));
  }
  fs.writeFileSync(rpcStatePath, `${JSON.stringify({
    apply: {
      updated_epoch: settlementEpoch - 1,
      pending_epoch: null,
      pending_next_page: 0,
      last_epoch_seconds: 3_600,
      last_apply_hash: null,
      last_page: null,
    },
    commit: staleSnapshotAndCommit ? {
      type: 'epoch_commit',
      epoch: settlementEpoch,
      status: 'provisional',
      at: 3_600,
      roots: {},
      totals: { use_count: 1 },
      commit_hash: COMMIT_HASH,
    } : null,
    superseded_commit: null,
    records,
    features: [],
    index_reads: 0,
    fail_page_once: failPageOnce,
    failed_once: false,
    drift_at_index_read: driftAtIndexRead,
    requested_keys: [],
  })}\n`);

  fs.writeFileSync(rpcServerPath, `
import fs from 'node:fs';
import http from 'node:http';
const [statePath, portPath] = process.argv.slice(2);
const read = () => JSON.parse(fs.readFileSync(statePath, 'utf8'));
const write = (state) => fs.writeFileSync(statePath, JSON.stringify(state));
const stable = (value) => Array.isArray(value)
  ? value.map(stable)
  : value && typeof value === 'object'
    ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, stable(value[key])]))
    : value;
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  const key = url.searchParams.get('key');
  if (req.method === 'GET' && url.pathname === '/v1/state') {
    const state = read();
    state.requested_keys.push(key);
    let value = null;
    if (key === 'epoch/apply/state') value = state.apply;
    else if (key === 'epoch/commit/${settlementEpoch}') value = state.commit;
    else {
      if (key === 'receipt/epoch/${settlementEpoch}/index') {
        state.index_reads += 1;
        if (state.drift_at_index_read === state.index_reads) {
          state.records[key] = {
            ...state.records[key],
            count: state.records[key].count + 1,
            revision: state.records[key].revision + 1,
            updated_at: '${'6'.repeat(64)}',
          };
        }
      }
      value = state.records[key] ?? null;
    }
    write(state);
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ key, confirmed: true, value }));
  }
  if (req.method === 'POST' && url.pathname === '/v1/contract/feature') {
    let raw = '';
    req.setEncoding('utf8');
    req.on('data', (chunk) => { raw += chunk; });
    return req.on('end', () => {
      const state = read();
      const feature = JSON.parse(raw);
      const value = feature.value;
      state.features.push(feature);
      const page = value.op === 'commit_apply_targeted_epoch_page0' ? 0 : value.page;
      const allocationAu = value.allocations.reduce((sum, entry) => sum + BigInt(entry.au), 0n);
      const debitAu = value.debits.reduce((sum, entry) => sum + BigInt(entry.au), 0n);
      const earningAu = value.earnings.reduce((sum, entry) => sum + BigInt(entry.gross_au), 0n);
      const index = state.records['receipt/epoch/${settlementEpoch}/index'];
      const errors = [];
      const writerOperation = {
        type: 'feature',
        key: 'mayhem_' + feature.key,
        value: {
          dispatch: {
            type: 'mayhem_feature',
            contract_version: Number.MAX_SAFE_INTEGER,
            key: feature.key,
            hash: '0'.repeat(128),
            value,
            nonce: '0'.repeat(64),
            address: '0'.repeat(64),
          },
        },
      };
      if (Buffer.byteLength(JSON.stringify(writerOperation)) > 64_000) errors.push('writer_bytes');
      if (page === 0 && value.op !== 'commit_apply_targeted_epoch_page0') errors.push('page0_op');
      if (page > 0 && value.op !== 'apply_targeted_epoch') errors.push('page_op');
      if (!/^[0-9a-f]{64}$/.test(value.epoch_commit_hash)) errors.push('commit');
      if (JSON.stringify(stable(value.receipt_index)) !== JSON.stringify(stable(index))) errors.push('index');
      if (allocationAu !== debitAu) errors.push('debits');
      if (allocationAu !== earningAu) errors.push('earnings');
      const hasFinalEvidence = value.last_page === true;
      if (hasFinalEvidence) {
        if (!Array.isArray(value.market_usage) || value.market_usage.length === 0 ||
            !Array.isArray(value.earning_finals) || value.earning_finals.length === 0) {
          errors.push('final_evidence');
        } else {
          const usageAu = value.market_usage.reduce(
            (sum, entry) => sum + BigInt(entry.demand_au),
            0n,
          );
          if (usageAu !== BigInt(index.count)) errors.push('market_usage');
        }
      } else if (value.market_usage !== undefined || value.earning_finals !== undefined) {
        errors.push('early_final_evidence');
      }
      if (page === 0) {
        if (!value.roots || !value.totals || value.totals.use_count !== index.count) {
          errors.push('commit_evidence');
        }
      } else if (!state.commit || value.epoch_commit_hash !== state.commit.commit_hash) {
        errors.push('missing_commit');
      }
      if (errors.length > 0) {
        write(state);
        res.writeHead(200, { 'content-type': 'application/json' });
        return res.end(JSON.stringify({ ok: false, error: 'fixture validation failed: ' + errors.join(',') }));
      }
      if (state.fail_page_once === page && !state.failed_once) {
        state.failed_once = true;
        write(state);
        res.writeHead(503, { 'content-type': 'application/json' });
        return res.end(JSON.stringify({ ok: false, error: 'forced page failure' }));
      }
      if (page === 0) {
        if (state.commit && state.commit.commit_hash !== value.epoch_commit_hash &&
            value.supersedes_commit_hash !== state.commit.commit_hash) {
          write(state);
          res.writeHead(200, { 'content-type': 'application/json' });
          return res.end(JSON.stringify({ ok: false, error: 'superseded commit mismatch' }));
        }
        if (state.commit && state.commit.commit_hash !== value.epoch_commit_hash) {
          state.superseded_commit = state.commit;
        }
        state.commit = {
          type: 'epoch_commit',
          epoch: ${settlementEpoch},
          epoch_seconds: 3600,
          apply_mode: 'targeted_receipt_pages_v1',
          status: 'provisional',
          at: value.at,
          roots: value.roots,
          totals: value.totals,
          commit_hash: value.epoch_commit_hash,
        };
      }
      if (value.last_page) {
        state.apply = {
          ...state.apply,
          updated_epoch: ${settlementEpoch},
          pending_epoch: null,
          pending_next_page: 0,
          last_apply_hash: '${APPLY_HASH}',
          last_page: page,
        };
      } else {
        state.apply = {
          ...state.apply,
          pending_epoch: ${settlementEpoch},
          pending_next_page: page + 1,
          pending_receipt_index_count: index.count,
          pending_receipt_index_revision: index.revision,
          pending_receipt_index_page_count: index.page_count,
          pending_receipt_index_updated_at: index.updated_at,
          last_apply_hash: '${APPLY_HASH}',
          last_page: page,
        };
      }
      write(state);
      res.writeHead(200, { 'content-type': 'application/json' });
      return res.end(JSON.stringify({ ok: true, page }));
    });
  }
  res.writeHead(404);
  res.end();
});
server.listen(0, '127.0.0.1', () => {
  fs.writeFileSync(portPath, String(server.address().port));
});
`, { mode: 0o600 });
  const server = spawn(process.execPath, [rpcServerPath, rpcStatePath, rpcPortPath], {
    stdio: ['ignore', 'ignore', 'inherit'],
  });
  for (let attempt = 0; attempt < 100 && !fs.existsSync(rpcPortPath); attempt += 1) {
    await sleep(10);
  }
  assert.equal(fs.existsSync(rpcPortPath), true, 'fixture RPC did not start');
  const port = fs.readFileSync(rpcPortPath, 'utf8').trim();

  writeExecutable(path.join(bin, 'flock'), '#!/usr/bin/env bash\nexit 0\n');
  writeExecutable(path.join(bin, 'mock-mayhem'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$MOCK_MAYHEM_LOG"
sim=0
recomputed=''
at=''
args=("$@")
for ((i=0; i<\${#args[@]}; i++)); do
  [[ "\${args[$i]}" == "--sim" ]] && sim=1
  [[ "\${args[$i]}" == "--recomputed-file" ]] && recomputed="\${args[$((i+1))]}"
  [[ "\${args[$i]}" == "--at" ]] && at="\${args[$((i+1))]}"
done
if (( sim == 0 )); then
  python3 - "$MOCK_RPC_STATE" "$recomputed" "$at" <<'PY'
import json, sys
state_path, recomputed_path, at = sys.argv[1:]
state = json.load(open(state_path))
recomputed = json.load(open(recomputed_path))
state["commit"] = {
    "type": "epoch_commit",
    "epoch": ${settlementEpoch},
    "status": "provisional",
    "at": int(at),
    "roots": recomputed["roots"],
    "totals": recomputed["totals"],
    "commit_hash": "${COMMIT_HASH}",
}
json.dump(state, open(state_path, "w"))
PY
fi
printf '%s\\n' '{"ok":true}'
`);
  const env = {
    PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`,
    HOME: path.join(root, 'home'),
    LANG: 'C',
    MAYHEM_PAYOUT_TEST_MODE: '1',
    MAYHEM_PAYOUT_TEST_ROOT: root,
    MAYHEM_BIN: path.join(bin, 'mock-mayhem'),
    MAYHEM_RPC_URL: `http://127.0.0.1:${port}/v1`,
    MAYHEM_ADMIN_HOME: home,
    MAYHEM_ADMIN_STORE: 'test-admin',
    MAYHEM_SOURCE_DIR: ROOT,
    MAYHEM_CADENCE_STATE_DIR: stateDir,
    MAYHEM_FINALIZER_STATE_WAIT_SECONDS: '2',
    MAYHEM_FINALIZER_CONFIRM_DELAY_SECONDS: '0',
    MOCK_RPC_STATE: rpcStatePath,
    MOCK_MAYHEM_LOG: mayhemLog,
  };
  return {
    root,
    stateDir,
    rpcStatePath,
    mayhemLog,
    env,
    run: () => spawnSync('bash', [SCRIPT, String(settlementEpoch)], {
      cwd: ROOT,
      env,
      encoding: 'utf8',
    }),
    state: () => JSON.parse(fs.readFileSync(rpcStatePath, 'utf8')),
    close: async () => {
      server.kill('SIGTERM');
      await new Promise((resolve) => server.once('exit', resolve));
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

test('finalizer atomically commits page zero then submits bounded exact targeted pages', async (t) => {
  const ctx = await harness();
  t.after(() => ctx.close());
  const result = ctx.run();
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  const state = ctx.state();
  assert.equal(state.apply.updated_epoch, 1);
  assert.equal(state.features.length, 2);
  assert.deepEqual(
    state.features.map((feature) =>
      feature.value.op === 'commit_apply_targeted_epoch_page0' ? 0 : feature.value.page
    ),
    [0, 1],
  );
  assert.deepEqual(
    state.features.map((feature) => feature.value.op),
    ['commit_apply_targeted_epoch_page0', 'apply_targeted_epoch'],
  );
  assert.deepEqual(state.features.map((feature) => feature.value.last_page), [false, true]);
  assert.deepEqual(state.features.map((feature) => feature.value.allocations.length), [2, 1]);
  assert.equal(state.features[0].value.roots !== undefined, true);
  assert.equal(state.features[0].value.totals !== undefined, true);
  assert.equal(state.features[1].value.roots, undefined);
  assert.equal(state.features[1].value.totals, undefined);
  assert.equal(state.commit.commit_hash, state.features[0].value.epoch_commit_hash);
  assert.equal(state.commit.apply_mode, 'targeted_receipt_pages_v1');
  assert.equal(state.features.every((feature) => feature.value.receipt_index.revision === 3), true);
  assert.equal(
    state.features.every((feature) =>
      feature.value.allocations.every((allocation) => allocation.billing_epoch === 1)
    ),
    true,
  );
  assert.equal(fs.existsSync(ctx.mayhemLog), false, 'page zero must not use a standalone commit');
  assert.equal(
    fs.existsSync(path.join(ctx.stateDir, 'cadence.last-advance')),
    false,
    'the cadence caller is the sole durable advance-stamp writer',
  );
});

test('finalizer settles a late billing receipt from the current receipt index', async (t) => {
  const ctx = await harness({ settlementEpoch: 2, billingEpoch: 1 });
  t.after(() => ctx.close());
  const result = ctx.run();
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  const state = ctx.state();
  assert.equal(state.apply.updated_epoch, 2);
  assert.equal(state.features.length, 2);
  assert.equal(
    state.features.every((feature) =>
      feature.value.epoch === 2 &&
      feature.value.receipt_index.epoch === 2 &&
      feature.value.allocations.every((allocation) => allocation.billing_epoch === 1)
    ),
    true,
  );
  const snapshot = JSON.parse(fs.readFileSync(
    path.join(ctx.stateDir, 'epochs/epoch-2/canonical-receipts.json'),
    'utf8',
  ));
  assert.equal(snapshot.settlement_epoch, 2);
  assert.equal(Object.hasOwn(snapshot, 'epoch'), false);
});

test('mid-page failure resumes the exact page and completed retry is idempotent', async (t) => {
  const ctx = await harness({ failPageOnce: 1 });
  t.after(() => ctx.close());
  const first = ctx.run();
  assert.notEqual(first.status, 0);
  assert.match(first.stderr, /retry resumes this exact page/);
  assert.equal(ctx.state().apply.pending_next_page, 1);
  const firstPageOne = ctx.state().features.find((feature) => feature.value.page === 1);

  const second = ctx.run();
  assert.equal(second.status, 0, `${second.stdout}\n${second.stderr}`);
  const after = ctx.state();
  assert.equal(after.apply.updated_epoch, 1);
  assert.deepEqual(
    after.features.map((feature) =>
      feature.value.op === 'commit_apply_targeted_epoch_page0' ? 0 : feature.value.page
    ),
    [0, 1, 1],
  );
  assert.deepEqual(
    after.features.filter((feature) => feature.value.page === 1),
    [firstPageOne, firstPageOne],
  );
  const featureCount = after.features.length;
  const third = ctx.run();
  assert.equal(third.status, 0, `${third.stdout}\n${third.stderr}`);
  assert.equal(ctx.state().features.length, featureCount);
  assert.match(third.stdout, /already finalized/);
});

test('metadata drift aborts before atomic targeted page zero starts', async (t) => {
  const ctx = await harness({ driftAtIndexRead: 4 });
  t.after(() => ctx.close());
  const result = ctx.run();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /metadata changed after snapshot/);
  assert.equal(ctx.state().features.length, 0);
  assert.equal(ctx.state().apply.updated_epoch, 0);
  assert.equal(ctx.state().apply.pending_epoch, null);
});

test('finalizer archives a stale snapshot and supersedes its unapplied commit', async (t) => {
  const ctx = await harness({ staleSnapshotAndCommit: true });
  t.after(() => ctx.close());
  const result = ctx.run();
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  assert.match(result.stdout, /archived stale unapplied epoch snapshot/);
  const state = ctx.state();
  assert.equal(state.apply.updated_epoch, 1);
  assert.equal(state.superseded_commit.commit_hash, COMMIT_HASH);
  assert.equal(state.features[0].value.supersedes_commit_hash, COMMIT_HASH);
  assert.notEqual(state.commit.commit_hash, COMMIT_HASH);
  const archiveRoot = path.join(ctx.stateDir, 'superseded/epoch-1');
  assert.equal(fs.readdirSync(archiveRoot).length, 1);
  assert.equal(
    fs.existsSync(path.join(archiveRoot, fs.readdirSync(archiveRoot)[0], 'canonical-receipts.json')),
    true,
  );
});

test('finalizer requires the exact receipt epoch index key and rejects the bare key', async (t) => {
  const ctx = await harness({ bareIndexOnly: true });
  t.after(() => ctx.close());
  const result = ctx.run();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /canonical state receipt\/epoch\/1\/index is missing/);
  const state = ctx.state();
  assert.equal(state.requested_keys.includes('receipt/epoch/1/index'), true);
  assert.equal(state.requested_keys.includes('receipt/epoch/1'), false);
  assert.equal(state.commit, null);
  assert.equal(state.features.length, 0);
  assert.equal(state.apply.updated_epoch, 0);
});
