#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIND="${MAYHEM_CANARY_SMOKE_BIND:-127.0.0.1:11436}"
GATEWAY_URL="http://$BIND"
KEEP_TMP="${MAYHEM_CANARY_SMOKE_KEEP_TMP:-0}"
TMP_DIR=""
GATEWAY_PID=""

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/dev-canary-mismatch-smoke.sh

Start a development embedded-catalog gateway, run mayhem auditor canary through
the normal gateway chat path with a deliberately wrong expected answer, and
assert that the resulting probe_result is pass:false with receipt/evidence
hashes.

This is local P4.5 canary mismatch evidence. It does not replace the formal
dev-net acceptance gate where a real deliberately mis-sealed provider/enclave is
caught and submitted on-contract.

Environment:
  MAYHEM_CANARY_SMOKE_BIND      Gateway bind address (default: 127.0.0.1:11436)
  MAYHEM_CANARY_SMOKE_KEEP_TMP  Keep temporary report/evidence/log files when 1
USAGE
}

cleanup() {
  if [[ -n "$GATEWAY_PID" ]] && kill -0 "$GATEWAY_PID" 2>/dev/null; then
    kill "$GATEWAY_PID" 2>/dev/null || true
    wait "$GATEWAY_PID" 2>/dev/null || true
  fi
  if [[ -n "$TMP_DIR" && "$KEEP_TMP" != "1" ]]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v cargo >/dev/null 2>&1 || die "cargo is required"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v node >/dev/null 2>&1 || die "node is required"

if curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1; then
  die "$GATEWAY_URL already has a Mayhem gateway; set MAYHEM_CANARY_SMOKE_BIND to another port"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-canary-mismatch.XXXXXX")"
GATEWAY_LOG="$TMP_DIR/gateway.log"
REPORT_JSON="$TMP_DIR/report.json"
EVIDENCE_JSON="$TMP_DIR/evidence.json"

log "starting development gateway on $GATEWAY_URL"
(
  cd "$ROOT_DIR"
  cargo run -q -p mayhem-gateway -- --dev-embedded-catalog --bind "$BIND"
) >"$GATEWAY_LOG" 2>&1 &
GATEWAY_PID="$!"

for _ in $(seq 1 120); do
  if curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
    sed -n '1,120p' "$GATEWAY_LOG" >&2 || true
    die "gateway exited before becoming ready"
  fi
  sleep 0.25
done

curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1 \
  || die "timed out waiting for $GATEWAY_URL/mayhem/status"

log "running deliberately failing auditor canary through gateway"
(
  cd "$ROOT_DIR"
  cargo run -q -p mayhem-cli -- auditor canary \
    --gateway-url "$GATEWAY_URL" \
    --epoch 7 \
    --at 42 \
    --probe-id local-canary-mismatch-smoke \
    --expected-text __mayhem_deliberately_wrong_canary_expected_text__ \
    --evidence-output "$EVIDENCE_JSON" \
    --json
) >"$REPORT_JSON"

node - "$REPORT_JSON" "$EVIDENCE_JSON" <<'NODE'
const fs = require('node:fs');
const [reportPath, evidencePath] = process.argv.slice(2);
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
const evidence = JSON.parse(fs.readFileSync(evidencePath, 'utf8'));
const hex64 = /^[0-9a-f]{64}$/i;

function assert(condition, message) {
  if (!condition) {
    console.error(`error: ${message}`);
    process.exit(1);
  }
}

assert(report.ok === false, 'canary report must fail for deliberately wrong expected text');
assert(report.evaluation?.pass === false, 'evaluation.pass must be false');
assert(report.probe_command?.pass === false, 'probe_command.pass must be false');
assert(report.probe_command?.probe_kind === 'canary', 'probe_command must be canary evidence');
assert(report.probe_command?.probe_id === 'local-canary-mismatch-smoke', 'probe id mismatch');
assert(report.probe_command?.match_bps < 9000, 'match_bps must be below canary threshold');
assert(hex64.test(report.probe_command?.session_receipt_hash ?? ''), 'missing receipt hash');
assert(hex64.test(report.probe_command?.evidence_hash ?? ''), 'missing evidence hash');
assert(report.probe_command.evidence_hash === report.evidence_hash, 'evidence hash mismatch');
assert(typeof report.provider === 'string' && report.provider.length > 0, 'missing provider');
assert(typeof report.enclave_id === 'string' && report.enclave_id.length > 0, 'missing enclave id');
assert(evidence.probe_command?.pass === false, 'evidence bundle must contain failing probe command');
assert(evidence.evidence?.latest_receipt, 'evidence bundle must retain latest gateway receipt');
console.log(JSON.stringify({
  ok: true,
  model: report.model?.id,
  prompt_id: report.canary?.prompt_id,
  match_bps: report.probe_command.match_bps,
  session_receipt_hash: report.probe_command.session_receipt_hash,
  evidence_hash: report.evidence_hash,
  report_path: reportPath,
  evidence_path: evidencePath,
}, null, 2));
NODE

log "local canary mismatch smoke passed"
if [[ "$KEEP_TMP" == "1" ]]; then
  log "kept temporary files in $TMP_DIR"
fi
