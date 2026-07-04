#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="$(date -u +%Y%m%dT%H%M%S)"
RUN_DIR="$ROOT_DIR/.mayhem-local/p4.5-canary/$TAG"
LOGS_DIR="$RUN_DIR/logs"
MODEL_ID="${MAYHEM_P45_MODEL:-qwen/qwen3.5-4b-gguf-q4_k_m-dev}"
CHUNK_SIZE="${MAYHEM_P45_CHUNK_SIZE:-8388608}"
KEEP_LOCAL="${MAYHEM_P45_KEEP_LOCAL:-1}"

MAYHEM_BIN="$ROOT_DIR/target/debug/mayhem"
ENCLAVE_BIN="$ROOT_DIR/target/debug/mayhem-enclave"

DEVNET_PID=""
PROVIDER_PID=""
GATEWAY_PID=""
DEVNET_LOG=""

log() {
  printf '[p4.5] %s\n' "$*" >&2
}

die() {
  printf '[p4.5] error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  set +e
  for pid in "$GATEWAY_PID" "$PROVIDER_PID" "$DEVNET_PID"; do
    if [[ -n "${pid:-}" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  if [[ -n "${DEVNET_LOG:-}" && -f "$DEVNET_LOG" ]]; then
    local stop_line
    stop_line="$(grep -E 'Stop them with: kill ' "$DEVNET_LOG" | tail -n 1 || true)"
    if [[ -n "$stop_line" ]]; then
      # shellcheck disable=SC2086
      kill ${stop_line#*kill } 2>/dev/null || true
    fi
  fi
  sleep 1
  for pid in "$GATEWAY_PID" "$PROVIDER_PID" "$DEVNET_PID"; do
    if [[ -n "${pid:-}" ]] && kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  if [[ -n "${DEVNET_LOG:-}" && -f "$DEVNET_LOG" ]]; then
    local stop_line
    stop_line="$(grep -E 'Stop them with: kill ' "$DEVNET_LOG" | tail -n 1 || true)"
    if [[ -n "$stop_line" ]]; then
      # shellcheck disable=SC2086
      kill -9 ${stop_line#*kill } 2>/dev/null || true
    fi
  fi
  if [[ "$KEEP_LOCAL" != "1" ]]; then
    rm -rf "$RUN_DIR"
  fi
}
trap cleanup EXIT

usage() {
  cat <<USAGE
Usage: scripts/dev-phase4-canary-smoke.sh

Runs the P4.5 formal dev-net canary acceptance:
  - starts a local Pear admin/user dev-net
  - seeds admin-created enclave, price, and room
  - starts a provider on the joiner Pear peer with the deterministic dev shim
  - accredits a separate auditor key on-contract
  - runs mayhem auditor canary through the normal contract-backed gateway
  - submits probeResult and proves the failed canary bans/tombstones the provider

Environment:
  MAYHEM_P45_MODEL       Catalog model id (default: $MODEL_ID)
  MAYHEM_P45_KEEP_LOCAL  Keep run evidence under .mayhem-local (default: 1)
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v cargo >/dev/null 2>&1 || die "cargo is required"
command -v node >/dev/null 2>&1 || die "node is required"
command -v curl >/dev/null 2>&1 || die "curl is required"

mkdir -p "$LOGS_DIR"
log "run dir: ${RUN_DIR#$ROOT_DIR/}"

free_port() {
  node - <<'NODE'
const net = require('node:net');
const server = net.createServer();
server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  server.close(() => {
    console.log(port);
  });
});
server.on('error', (err) => {
  console.error(err.message);
  process.exit(1);
});
NODE
}

json_field() {
  local file="$1"
  local expr="$2"
  node - "$file" "$expr" <<'NODE'
const fs = require('node:fs');
const [file, expr] = process.argv.slice(2);
const value = JSON.parse(fs.readFileSync(file, 'utf8'));
const parts = expr.split('.');
let cursor = value;
for (const part of parts) {
  if (cursor == null) break;
  cursor = cursor[part];
}
if (cursor === undefined || cursor === null) process.exit(2);
if (typeof cursor === 'object') console.log(JSON.stringify(cursor));
else console.log(String(cursor));
NODE
}

wait_for_file_pattern() {
  local file="$1"
  local pattern="$2"
  local label="$3"
  local timeout_seconds="$4"
  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if [[ -f "$file" ]] && grep -Eq "$pattern" "$file"; then
      return 0
    fi
    sleep 0.5
  done
  if [[ -f "$file" ]]; then
    tail -n 120 "$file" >&2 || true
  fi
  die "timed out waiting for $label"
}

wait_http() {
  local url="$1"
  local label="$2"
  local timeout_seconds="$3"
  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  die "timed out waiting for $label at $url"
}

parse_devnet() {
  node - "$DEVNET_LOG" <<'NODE'
const fs = require('node:fs');
const text = fs.readFileSync(process.argv[2], 'utf8');
const admin = text.match(/admin:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
const joiner = text.match(/joiner-a:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
const token = text.match(/sc bridge token:\s+(\S+)/);
const logs = text.match(/logs:\s+(\S+)/);
if (!admin || !joiner || !token || !logs) {
  console.error('could not parse dev-net log');
  process.exit(1);
}
const adminLog = fs.readFileSync(`${logs[1]}/admin.log`, 'utf8');
const adminPubkey = adminLog.match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/i);
if (!adminPubkey) {
  console.error('could not parse admin pubkey');
  process.exit(1);
}
console.log(JSON.stringify({
  admin_ws: admin[1],
  admin_rpc: admin[2],
  joiner_ws: joiner[1],
  joiner_rpc: joiner[2],
  token: token[1],
  admin_pubkey: adminPubkey[1],
  logs: logs[1],
}, null, 2));
NODE
}

prepare_catalog() {
  local admin_pubkey="$1"
  local binary_hash="$2"
  local artifact_path="$3"
  local catalog_out="$4"
  node - "$ROOT_DIR" "$MODEL_ID" "$admin_pubkey" "$binary_hash" "$artifact_path" "$catalog_out" "$CHUNK_SIZE" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { createRequire } = require('node:module');

const [root, modelId, adminPubkey, binaryHash, artifactPath, catalogOut, chunkSizeRaw] = process.argv.slice(2);
const requireFromRoot = createRequire(path.join(root, 'scripts/dev-phase4-canary-smoke.sh'));
const { blake3 } = requireFromRoot(path.join(root, 'intercom/node_modules/@tracsystems/blake3'));
const chunkSize = Number(chunkSizeRaw);

function u64le(value) {
  const out = Buffer.alloc(8);
  out.writeBigUInt64LE(BigInt(value));
  return out;
}

async function b3(buffer) {
  return Buffer.from(await blake3(buffer));
}

async function merkleLeaf(index, len, data) {
  return b3(Buffer.concat([
    Buffer.from('mayhem-blake3-merkle-v1:leaf'),
    u64le(index),
    u64le(len),
    data,
  ]));
}

async function merkleParent(left, right) {
  return b3(Buffer.concat([
    Buffer.from('mayhem-blake3-merkle-v1:node'),
    left,
    right,
  ]));
}

async function merkleRoot(file) {
  const fd = fs.openSync(file, 'r');
  const buffer = Buffer.allocUnsafe(chunkSize);
  const leaves = [];
  let totalBytes = 0;
  let index = 0;
  try {
    for (;;) {
      const read = fs.readSync(fd, buffer, 0, buffer.length, null);
      if (read === 0) break;
      totalBytes += read;
      leaves.push(await merkleLeaf(index, read, buffer.subarray(0, read)));
      index += 1;
    }
  } finally {
    fs.closeSync(fd);
  }
  if (leaves.length === 0) {
    return { root: (await b3(Buffer.from('mayhem-blake3-merkle-v1:empty'))).toString('hex'), chunks: 0, total_bytes: 0 };
  }
  let layer = leaves;
  while (layer.length > 1) {
    const next = [];
    for (let i = 0; i < layer.length; i += 2) {
      next.push(await merkleParent(layer[i], layer[i + 1] || layer[i]));
    }
    layer = next;
  }
  return { root: layer[0].toString('hex'), chunks: leaves.length, total_bytes: totalBytes };
}

(async () => {
  const catalog = JSON.parse(fs.readFileSync(path.join(root, 'catalog/models.json'), 'utf8'));
  const model = catalog.models.find((entry) => entry.model_id === modelId);
  if (!model) throw new Error(`catalog model not found: ${modelId}`);
  const artifactEntry = Object.entries(model.artifacts).find(([, artifact]) => artifact.engine === 'llama.cpp');
  if (!artifactEntry) throw new Error(`model ${modelId} has no llama.cpp artifact`);
  const [artifactName, artifact] = artifactEntry;
  const merkle = await merkleRoot(artifactPath);
  const artifactSha256 = crypto.createHash('sha256').update(fs.readFileSync(artifactPath)).digest('hex');
  artifact.artifact_root = merkle.root;
  artifact.artifact_root_kind = 'blake3_merkle_v1';
  artifact.weights_bytes = merkle.total_bytes;
  artifact.source_sha256 = artifactSha256;
  fs.mkdirSync(path.dirname(catalogOut), { recursive: true });
  fs.writeFileSync(catalogOut, `${JSON.stringify(catalog, null, 2)}\n`);
  const manifestHash = crypto.createHash('sha256').update(fs.readFileSync(catalogOut)).digest('hex');
  const enclaveId = (await b3(Buffer.from(`${adminPubkey}${modelId}${merkle.root}${manifestHash}${binaryHash}`, 'utf8'))).toString('hex');
  console.log(JSON.stringify({
    model_id: modelId,
    artifact_name: artifactName,
    artifact_root: merkle.root,
    artifact_root_kind: artifact.artifact_root_kind,
    artifact_source: artifact.source,
    artifact_path: artifact.path,
    artifact_chunks: merkle.chunks,
    artifact_bytes: merkle.total_bytes,
    artifact_sha256: artifactSha256,
    manifest_hash: manifestHash,
    binary_hash: binaryHash,
    enclave_id: enclaveId,
  }, null, 2));
})().catch((err) => {
  console.error(err.stack || err.message || String(err));
  process.exit(1);
});
NODE
}

log "building mayhem CLI/gateway/enclave crates"
cargo build -q -p mayhem-cli -p mayhem-gateway -p mayhem-enclave
[[ -x "$MAYHEM_BIN" ]] || die "missing $MAYHEM_BIN"
[[ -x "$ENCLAVE_BIN" ]] || die "missing $ENCLAVE_BIN"

log "starting local Pear dev-net"
DEVNET_LOG="$LOGS_DIR/dev-net.log"
MAYHEM_DEVNET_JOINERS=1 \
MAYHEM_DEVNET_SUBNET_CHANNEL="mayhem-p45-$TAG" \
MAYHEM_DEVNET_REPLICATE_FLUSH_TIMEOUT_MS=5000 \
SESSION_DEBUG=1 \
SC_BRIDGE_DEBUG="${SC_BRIDGE_DEBUG:-1}" \
  "$ROOT_DIR/scripts/dev-net.sh" --cleanup --keep-running >"$DEVNET_LOG" 2>&1 &
DEVNET_PID="$!"
wait_for_file_pattern "$DEVNET_LOG" 'Mayhem dev-net ready\.' 'local dev-net' 180
DEVNET_JSON="$RUN_DIR/devnet.json"
parse_devnet >"$DEVNET_JSON"

ADMIN_WS="$(json_field "$DEVNET_JSON" admin_ws)"
ADMIN_RPC="$(json_field "$DEVNET_JSON" admin_rpc)"
JOINER_WS="$(json_field "$DEVNET_JSON" joiner_ws)"
JOINER_RPC="$(json_field "$DEVNET_JSON" joiner_rpc)"
SC_TOKEN="$(json_field "$DEVNET_JSON" token)"
ADMIN_PUBKEY="$(json_field "$DEVNET_JSON" admin_pubkey)"

ADMIN_HOME="$RUN_DIR/admin-home"
PROVIDER_HOME="$RUN_DIR/provider-home"
AUDITOR_HOME="$RUN_DIR/auditor-home"
GATEWAY_HOME="$RUN_DIR/gateway-home"
mkdir -p "$ADMIN_HOME/stores" "$PROVIDER_HOME/stores" "$AUDITOR_HOME" "$GATEWAY_HOME"
ln -s "$ROOT_DIR/intercom/stores/mayhem-devnet-admin" "$ADMIN_HOME/stores/admin"
ln -s "$ROOT_DIR/intercom/stores/mayhem-devnet-joiner-a" "$PROVIDER_HOME/stores/main"

RULES_JSON="$RUN_DIR/rules-hash.json"
"$MAYHEM_BIN" rules hash --print-json >"$RULES_JSON"
RULES_HASH="$(json_field "$RULES_JSON" hash)"

ARTIFACT="$RUN_DIR/p45-shim-artifact.bin"
printf 'mayhem phase4 canary deterministic artifact\n%s\n' "$TAG" >"$ARTIFACT"
BINARY_HASH="$("$ENCLAVE_BIN" measure-binary --binary "$MAYHEM_BIN" | sed 's/^binary_hash=//')"
TEMP_CATALOG="$RUN_DIR/catalog/models.json"
CATALOG_META="$RUN_DIR/catalog-meta.json"
prepare_catalog "$ADMIN_PUBKEY" "$BINARY_HASH" "$ARTIFACT" "$TEMP_CATALOG" >"$CATALOG_META"
ENCLAVE_ID="$(json_field "$CATALOG_META" enclave_id)"
ARTIFACT_ROOT="$(json_field "$CATALOG_META" artifact_root)"
ARTIFACT_ROOT_KIND="$(json_field "$CATALOG_META" artifact_root_kind)"
ARTIFACT_REPO="$(json_field "$CATALOG_META" artifact_source.repo)"
ARTIFACT_REVISION="$(json_field "$CATALOG_META" artifact_source.revision)"
ARTIFACT_PATH="$(json_field "$CATALOG_META" artifact_path)"
ARTIFACT_SHA256="$(json_field "$CATALOG_META" artifact_sha256)"
MANIFEST_HASH="$(json_field "$CATALOG_META" manifest_hash)"

log "seeding admin rules, enclave, price, and room"
admin_run() {
  local name="$1"
  shift
  "$MAYHEM_BIN" admin "$@" \
    --home "$ADMIN_HOME" \
    --peer-store-name admin \
    --rpc-url "$ADMIN_RPC" \
    --submit \
    --json >"$RUN_DIR/$name.json"
}
admin_run admin-set-rules set-rules --ver 1 --hash "$RULES_HASH"
admin_run admin-set-params set-params \
  --submitted-at 0 \
  --effective-at 86400 \
  --values-json '{"fee_bps":1500,"holdback_epochs":0,"challenge_epochs":0,"payout_min_mu":0,"rate_staleness_seconds":86400,"canary_match_min_bps":9000,"probe_reward_mu":5000,"uptime_tick_seconds":21600}'
admin_run admin-set-model-ref set-model-ref \
  --model "$MODEL_ID" \
  --in-per-1k-mu 18 \
  --out-per-1k-mu 55
admin_run admin-register-enclave register-enclave \
  --enclave-id "$ENCLAVE_ID" \
  --model "$MODEL_ID" \
  --backend llama.cpp \
  --artifact-root "$ARTIFACT_ROOT" \
  --artifact-root-kind "$ARTIFACT_ROOT_KIND" \
  --artifact-repo "$ARTIFACT_REPO" \
  --artifact-revision "$ARTIFACT_REVISION" \
  --artifact-path "$ARTIFACT_PATH" \
  --source-sha256 "$ARTIFACT_SHA256" \
  --catalog-path "$TEMP_CATALOG" \
  --dev-skip-catalog-verify \
  --manifest-hash "$MANIFEST_HASH" \
  --binary-hash "$BINARY_HASH" \
  --caps-json '{"chat":true,"tools":true,"json":true,"ctx":8192}'
admin_run admin-set-price set-price \
  --enclave-id "$ENCLAVE_ID" \
  --in-per-1k-mu 18 \
  --out-per-1k-mu 55 \
  --effective-at 0
admin_run admin-open-room open-room \
  --enclave-id "$ENCLAVE_ID" \
  --model "$MODEL_ID" \
  --nonce "p4.5-$TAG" \
  --label phase4-canary

log "consenting provider wallet and starting provider sessions on joiner peer"
"$MAYHEM_BIN" setup \
  --home "$PROVIDER_HOME" \
  --role provider \
  --wallet reuse \
  --peer-store-name main \
  --rpc-url "$ADMIN_RPC" \
  --rules-ver 1 \
  --rules-hash "$RULES_HASH" \
  --yes \
  --print-json >"$RUN_DIR/provider-setup.json"

PROVIDER_LOG="$LOGS_DIR/provider-start.log"
MAYHEM_PROVIDER_SESSION_DEBUG=1 "$MAYHEM_BIN" provider start \
  --home "$PROVIDER_HOME" \
  --enclave "$ENCLAVE_ID" \
  --rpc-url "$ADMIN_RPC" \
  --session-rpc-url "$ADMIN_RPC" \
  --sc-bridge-url "$JOINER_WS" \
  --sc-bridge-token "$SC_TOKEN" \
  --catalog-path "$TEMP_CATALOG" \
  --artifact "$ARTIFACT" \
  --engine-backend llama.cpp \
  --skip-disk-bench \
  --chunk-size "$CHUNK_SIZE" \
  --serve-sessions \
  --serve-sessions-seconds 900 \
  --print-json \
  --dev-skip-catalog-verify \
  --dev-session-shim >"$PROVIDER_LOG" 2>&1 &
PROVIDER_PID="$!"
wait_for_file_pattern "$PROVIDER_LOG" '"self_test"' 'provider start self-test' 240
PROVIDER_JSON="$RUN_DIR/provider-start.json"
node - "$PROVIDER_LOG" >"$PROVIDER_JSON" <<'NODE'
const fs = require('node:fs');
const text = fs.readFileSync(process.argv[2], 'utf8');
const start = text.indexOf('{');
if (start < 0) {
  console.error('provider log did not contain JSON');
  process.exit(1);
}
let depth = 0;
let inString = false;
let escape = false;
for (let i = start; i < text.length; i += 1) {
  const ch = text[i];
  if (inString) {
    if (escape) escape = false;
    else if (ch === '\\') escape = true;
    else if (ch === '"') inString = false;
    continue;
  }
  if (ch === '"') inString = true;
  else if (ch === '{') depth += 1;
  else if (ch === '}') {
    depth -= 1;
    if (depth === 0) {
      console.log(JSON.stringify(JSON.parse(text.slice(start, i + 1)), null, 2));
      process.exit(0);
    }
  }
}
console.error('provider JSON was incomplete');
process.exit(1);
NODE
PROVIDER_PUBKEY="$(json_field "$PROVIDER_JSON" provider)"
wait_for_file_pattern "$PROVIDER_LOG" 'session frame subscription ready' 'provider session server' 60

log "creating and accrediting auditor wallet"
"$MAYHEM_BIN" setup \
  --home "$AUDITOR_HOME" \
  --role user \
  --wallet create \
  --rpc-url "$ADMIN_RPC" \
  --rules-ver 1 \
  --rules-hash "$RULES_HASH" \
  --yes \
  --print-json >"$RUN_DIR/auditor-setup.json"
AUDITOR_PUBKEY="$(json_field "$RUN_DIR/auditor-setup.json" wallet.public_key)"
admin_run admin-auditor-register auditor-register \
  --auditor "$AUDITOR_PUBKEY" \
  --registered-at-seconds 0

log "starting contract-backed gateway on admin peer"
GATEWAY_PORT="$(free_port)"
GATEWAY_URL="http://127.0.0.1:$GATEWAY_PORT"
GATEWAY_LOG="$LOGS_DIR/gateway.log"
"$MAYHEM_BIN" use \
  --home "$GATEWAY_HOME" \
  --rpc-url "$ADMIN_RPC" \
  --sc-bridge-url "$ADMIN_WS" \
  --sc-bridge-token "$SC_TOKEN" \
  --session-open-timeout-seconds 30 \
  --session-frame-timeout-seconds 15 \
  --bind "127.0.0.1:$GATEWAY_PORT" \
  --json >"$GATEWAY_LOG" 2>&1 &
GATEWAY_PID="$!"
wait_http "$GATEWAY_URL/mayhem/status" 'contract-backed gateway' 120

log "waiting for gateway to expose the provider route"
node - "$GATEWAY_URL" "$MODEL_ID" "$PROVIDER_PUBKEY" <<'NODE'
const [gatewayUrl, modelId, provider] = process.argv.slice(2);
(async () => {
  const deadline = Date.now() + 120_000;
  let last = null;
  while (Date.now() < deadline) {
    try {
      const models = await (await fetch(`${gatewayUrl}/v1/models`)).json();
      const selected = models.data?.find((entry) => entry.id === modelId);
      const routes = selected?.mayhem?.route_candidates || [];
      if (routes.some((route) => route.provider === provider)) {
        process.exit(0);
      }
      last = { selected: Boolean(selected), routes };
    } catch (err) {
      last = { error: err.message };
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  console.error(`gateway did not expose provider route: ${JSON.stringify(last)}`);
  process.exit(1);
})().catch((err) => {
  console.error(err.stack || err.message || String(err));
  process.exit(1);
});
NODE

log "running failing auditor canary through normal gateway path and submitting probeResult"
PROBE_ID="p45-canary-$TAG"
CANARY_REPORT="$RUN_DIR/canary-report.json"
CANARY_EVIDENCE="$RUN_DIR/canary-evidence.json"
"$MAYHEM_BIN" auditor canary \
  --home "$AUDITOR_HOME" \
  --gateway-url "$GATEWAY_URL" \
  --rpc-url "$ADMIN_RPC" \
  --model "$MODEL_ID" \
  --canary-set canary-dev-v1 \
  --prompt-id dev-arithmetic-json \
  --expected-text '{"answer":42}' \
  --epoch 1 \
  --at 10000 \
  --probe-id "$PROBE_ID" \
  --evidence-output "$CANARY_EVIDENCE" \
  --submit \
  --json >"$CANARY_REPORT"

log "verifying submitted probe/slash state"
VERIFY_REPORT="$RUN_DIR/verify-report.json"
node - "$ADMIN_RPC" "$PROBE_ID" "$PROVIDER_PUBKEY" "$AUDITOR_PUBKEY" "$CANARY_REPORT" "$CANARY_EVIDENCE" >"$VERIFY_REPORT" <<'NODE'
const fs = require('node:fs');
const [rpcUrl, probeId, provider, auditor, reportPath, evidencePath] = process.argv.slice(2);
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
const evidence = JSON.parse(fs.readFileSync(evidencePath, 'utf8'));
const hex64 = /^[0-9a-f]{64}$/i;

async function state(key) {
  const url = new URL(`${rpcUrl.replace(/\/$/, '')}/state`);
  url.searchParams.set('key', key);
  url.searchParams.set('confirmed', 'false');
  const response = await fetch(url);
  if (!response.ok) throw new Error(`state ${key} failed: ${response.status}`);
  return (await response.json()).value ?? null;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

(async () => {
  const probe = await state(`ev/probe/${probeId}`);
  const providerState = await state(`prov/${provider}`);
  const auditorState = await state(`auditor/${auditor}`);
  const serveState = await state(`serve/${provider}/${report.enclave_id}`);

  assert(report.ok === false, 'canary report must fail');
  assert(report.evaluation?.pass === false, 'evaluation.pass must be false');
  assert(report.probe_command?.pass === false, 'probe command must be failing');
  assert(hex64.test(report.submitted?.tx || ''), 'probeResult submit tx must be recorded');
  assert(hex64.test(report.submitted?.command_hash || ''), 'probeResult command hash must be recorded');
  assert(hex64.test(report.probe_command?.session_receipt_hash || ''), 'missing session receipt hash');
  assert(hex64.test(report.probe_command?.evidence_hash || ''), 'missing evidence hash');
  assert(evidence.evidence?.latest_receipt, 'evidence bundle must retain the paid-session receipt');
  assert(probe?.probe_id === probeId, 'probe state missing');
  assert(probe.pass === false, 'probe state must fail');
  assert(probe.provenance_violation === true, 'probe state must mark provenance violation');
  assert(probe.session_receipt_hash === report.probe_command.session_receipt_hash, 'receipt hash mismatch');
  assert(probe.evidence_hash === report.probe_command.evidence_hash, 'evidence hash mismatch');
  assert(probe.slash?.reason === 'canary_mismatch', 'slash reason must be canary_mismatch');
  assert(probe.slash?.provider_banned === true, 'slash must ban provider');
  assert(providerState?.status === 'banned', 'provider must be banned');
  assert(Array.isArray(providerState?.enclaves) && providerState.enclaves.length === 0, 'provider active enclaves must be cleared');
  assert(auditorState?.submitted_probes === 1, 'auditor submitted_probes must increment');
  assert((auditorState?.successful_probes ?? 0) === 0, 'failed canary must not increment successful_probes');
  assert(serveState?.status === 'tombstoned', 'serve row must be tombstoned');

  console.log(JSON.stringify({
    ok: true,
    probe,
    provider: providerState,
    auditor: auditorState,
    serve: serveState,
  }, null, 2));
})().catch((err) => {
  console.error(err.stack || err.message || String(err));
  process.exit(1);
});
NODE

REPORT="$RUN_DIR/report.json"
node - "$RUN_DIR" "$TAG" "$MODEL_ID" "$ENCLAVE_ID" "$PROVIDER_PUBKEY" "$AUDITOR_PUBKEY" "$GATEWAY_URL" "$CANARY_REPORT" "$CANARY_EVIDENCE" "$VERIFY_REPORT" "$CATALOG_META" <<'NODE'
const fs = require('node:fs');
const [runDir, tag, modelId, enclaveId, provider, auditor, gatewayUrl, canaryReportPath, canaryEvidencePath, verifyReportPath, catalogMetaPath] = process.argv.slice(2);
const canary = JSON.parse(fs.readFileSync(canaryReportPath, 'utf8'));
const verify = JSON.parse(fs.readFileSync(verifyReportPath, 'utf8'));
const catalog = JSON.parse(fs.readFileSync(catalogMetaPath, 'utf8'));
const report = {
  ok: true,
  tag,
  run_dir: runDir.replace(`${process.cwd()}/`, ''),
  gateway_url: gatewayUrl,
  model_id: modelId,
  enclave_id: enclaveId,
  provider,
  auditor,
  artifact_root: catalog.artifact_root,
  manifest_hash: catalog.manifest_hash,
  binary_hash: catalog.binary_hash,
  canary: {
    probe_id: canary.probe_command.probe_id,
    canary_set: canary.canary.set_id,
    prompt_id: canary.canary.prompt_id,
    match_bps: canary.probe_command.match_bps,
    pass: canary.probe_command.pass,
    session_receipt_hash: canary.probe_command.session_receipt_hash,
    evidence_hash: canary.probe_command.evidence_hash,
    evidence_path: canaryEvidencePath,
  },
  submitted: canary.submitted,
  contract_state: {
    probe_pass: verify.probe.pass,
    provenance_violation: verify.probe.provenance_violation,
    slash_reason: verify.probe.slash.reason,
    provider_status: verify.provider.status,
    serve_status: verify.serve.status,
    auditor_submitted_probes: verify.auditor.submitted_probes,
    auditor_successful_probes: verify.auditor.successful_probes,
  },
  assertions: {
    normal_gateway_receipt_bound: true,
    probe_result_submitted: true,
    failed_canary_recorded: true,
    provider_banned: true,
    serve_tombstoned: true,
  },
};
fs.writeFileSync(`${runDir}/report.json`, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
NODE

log "P4.5 canary smoke passed"
