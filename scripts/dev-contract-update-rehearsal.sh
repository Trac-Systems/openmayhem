#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR=""
DEVNET_PID=""
DEVNET_PEER_PIDS=""
KEEP_TMP="${MAYHEM_CONTRACT_UPDATE_REHEARSAL_KEEP_TMP:-0}"
SKIP_CARGO="${MAYHEM_CONTRACT_UPDATE_REHEARSAL_SKIP_CARGO:-0}"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/dev-contract-update-rehearsal.sh

Runs the local contract-update rehearsal gate:
  - JS contract tests for versioned signing, current receipt schema, and param delay;
  - Rust handshake tests for explicit upgrade-required failures;
  - a fresh Pear dev-net where simulated noop reports the current contract version.

Environment:
  MAYHEM_CONTRACT_UPDATE_REHEARSAL_SKIP_CARGO=1 skips Rust tests.
  MAYHEM_CONTRACT_UPDATE_REHEARSAL_KEEP_TMP=1 keeps temporary logs.
USAGE
}

cleanup() {
  if [[ -n "$DEVNET_PEER_PIDS" ]]; then
    # shellcheck disable=SC2086
    kill $DEVNET_PEER_PIDS 2>/dev/null || true
    # shellcheck disable=SC2086
    wait $DEVNET_PEER_PIDS 2>/dev/null || true
  fi
  if [[ -n "$DEVNET_PID" ]] && kill -0 "$DEVNET_PID" 2>/dev/null; then
    kill "$DEVNET_PID" 2>/dev/null || true
    wait "$DEVNET_PID" 2>/dev/null || true
  fi
  if [[ -n "$TMP_DIR" && "$KEEP_TMP" != "1" ]]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT INT TERM

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v node >/dev/null 2>&1 || die "node is required"
if [[ "$SKIP_CARGO" != "1" ]]; then
  command -v cargo >/dev/null 2>&1 || die "cargo is required"
fi

CONTRACT_VERSION="$(
  cd "$ROOT_DIR"
  node --input-type=module -e 'import { CONTRACT_VERSION } from "./intercom/contract/contract.js"; console.log(CONTRACT_VERSION);'
)"
[[ "$CONTRACT_VERSION" =~ ^[0-9]+$ ]] || die "could not read CONTRACT_VERSION"

log "running JS contract versioning and activation-delay tests"
(
  cd "$ROOT_DIR"
  node --test \
    intercom/tests/contract-versioning.test.js \
    intercom/tests/contract-params.test.js \
    intercom/tests/sparse-contract-transition.test.js
)

if [[ "$SKIP_CARGO" != "1" ]]; then
  RUST_CHECK="rust-handshake-tests"
  log "running Rust contract handshake tests"
  (
    cd "$ROOT_DIR"
    cargo test -p mayhem-cli provider_session_open_enforces_admin_terms -- --nocapture
    cargo test -p mayhem-gateway heartbeat_without_contract_version_is_rejected_before_signature_check -- --nocapture
    cargo test -p mayhem-gateway direct_session_accept_pins_session_enclave_provider_and_signature -- --nocapture
  )
else
  RUST_CHECK="rust-handshake-tests-skipped"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-contract-update.XXXXXX")"
DEVNET_LOG="$TMP_DIR/dev-net.log"

log "starting fresh local Pear dev-net"
(
  cd "$ROOT_DIR"
  MAYHEM_DEVNET_JOINERS=1 scripts/dev-net.sh --cleanup --keep-running
) >"$DEVNET_LOG" 2>&1 &
DEVNET_PID="$!"

for _ in $(seq 1 720); do
  if grep -q "Mayhem dev-net ready." "$DEVNET_LOG" 2>/dev/null; then
    break
  fi
  if ! kill -0 "$DEVNET_PID" 2>/dev/null; then
    sed -n '1,160p' "$DEVNET_LOG" >&2 || true
    die "dev-net exited before becoming ready"
  fi
  sleep 0.5
done
grep -q "Mayhem dev-net ready." "$DEVNET_LOG" || die "timed out waiting for dev-net"

ADMIN_WS="$(awk '/admin:/ { print $2; exit }' "$DEVNET_LOG")"
TOKEN="$(awk -F':  +' '/sc bridge token:/ { print $2; exit }' "$DEVNET_LOG")"
DEVNET_PEER_PIDS="$(sed -n 's/^Peers are still running\. Stop them with: kill //p' "$DEVNET_LOG" | tail -1)"
[[ -n "$ADMIN_WS" && -n "$TOKEN" ]] || die "could not parse dev-net endpoints"
ADMIN_PORT="${ADMIN_WS##*:}"

bridge_request() {
  local port="$1"
  local payload="$2"
  node - "$port" "$TOKEN" "$payload" <<'NODE'
const [port, token, rawPayload] = process.argv.slice(2);
const payload = JSON.parse(rawPayload);
const ws = new WebSocket(`ws://127.0.0.1:${port}`);
let sent = false;
const timer = setTimeout(() => {
  console.error(`SC-Bridge request timed out on ${port}`);
  process.exit(1);
}, 10000);

function finish(code, value) {
  clearTimeout(timer);
  if (value !== undefined) {
    const out = typeof value === 'string' ? value : JSON.stringify(value);
    if (code === 0) console.log(out);
    else console.error(out);
  }
  try { ws.close(); } catch {}
  process.exit(code);
}

ws.onerror = (error) => finish(1, error?.message || String(error));
ws.onopen = () => ws.send(JSON.stringify({ id: 1, type: 'auth', token }));
ws.onmessage = (event) => {
  let msg;
  try {
    msg = JSON.parse(event.data);
  } catch {
    return;
  }
  if (msg.id === 1 && msg.type === 'auth_ok') {
    payload.id = 2;
    sent = true;
    ws.send(JSON.stringify(payload));
    return;
  }
  if (msg.id === 1 && msg.type === 'error') finish(1, msg);
  if (sent && msg.id === 2) {
    if (msg.type === 'error') finish(1, msg);
    else finish(0, msg);
  }
};
NODE
}

log "checking live dev-net contract version"
NOOP_RESULT="$(bridge_request "$ADMIN_PORT" '{"type":"cli","command":"/tx --command \"noop\" --sim 1"}')"
node - "$NOOP_RESULT" "$CONTRACT_VERSION" <<'NODE'
const [raw, expectedRaw] = process.argv.slice(2);
const expected = Number(expectedRaw);
const message = JSON.parse(raw);
const result = message.result?.result ?? message.result;
if (message.type !== 'cli_result' || message.ok !== true || !result || result.ok !== true) {
  console.error(raw);
  process.exit(1);
}
if (result.op !== 'noop' || result.version !== expected) {
  console.error(`expected noop CONTRACT_VERSION ${expected}, got ${JSON.stringify(result)}`);
  process.exit(1);
}
NODE

REPORT="$TMP_DIR/report.json"
node - "$REPORT" "$CONTRACT_VERSION" "$DEVNET_LOG" "$RUST_CHECK" <<'NODE'
const fs = require('node:fs');
const [reportPath, contractVersion, devnetLog, rustCheck] = process.argv.slice(2);
fs.writeFileSync(reportPath, JSON.stringify({
  ok: true,
  contract_version: Number(contractVersion),
  checks: [
    'contract-versioning.test.js',
    'contract-params.test.js',
    'sparse-contract-transition.test.js',
    rustCheck,
    'pear-dev-net-noop-version'
  ],
  devnet_log: devnetLog
}, null, 2) + '\n');
NODE

cat "$REPORT"
log "contract update rehearsal passed"
