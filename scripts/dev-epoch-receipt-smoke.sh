#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIND="${MAYHEM_EPOCH_SMOKE_BIND:-127.0.0.1:11438}"
GATEWAY_URL="http://$BIND"
EPOCH="${MAYHEM_EPOCH_SMOKE_EPOCH:-7}"
FEE_BPS="${MAYHEM_EPOCH_SMOKE_FEE_BPS:-1500}"
KEEP_TMP="${MAYHEM_EPOCH_SMOKE_KEEP_TMP:-0}"
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
Usage: scripts/dev-epoch-receipt-smoke.sh

Start a development embedded-catalog gateway, generate real gateway receipts via
mayhem test, export an epoch audit bundle with explicit admin fee_bps,
independently recompute dep/use/earn/fee/pay roots, synthesize the matching ev/*
snapshot, and verify the receipt export against all ev/* checks.

This is local P5.4/P8.5 settlement evidence. It does not replace the formal
dev-net gate where provider/user P2P receipts are collected over mx/epoch and
settled on-contract.

Environment:
  MAYHEM_EPOCH_SMOKE_BIND      Gateway bind address (default: 127.0.0.1:11438)
  MAYHEM_EPOCH_SMOKE_EPOCH     Epoch number for the smoke (default: 7)
  MAYHEM_EPOCH_SMOKE_FEE_BPS   Admin fee split in bps (default: 1500)
  MAYHEM_EPOCH_SMOKE_KEEP_TMP  Keep temporary report/evidence/log files when 1
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

if ! [[ "$EPOCH" =~ ^[1-9][0-9]*$ ]]; then
  die "MAYHEM_EPOCH_SMOKE_EPOCH must be a positive integer"
fi
if ! [[ "$FEE_BPS" =~ ^[0-9]+$ ]] || (( FEE_BPS > 5000 )); then
  die "MAYHEM_EPOCH_SMOKE_FEE_BPS must be an integer from 0 to 5000"
fi
if curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1; then
  die "$GATEWAY_URL already has a Mayhem gateway; set MAYHEM_EPOCH_SMOKE_BIND to another port"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-epoch-smoke.XXXXXX")"
GATEWAY_LOG="$TMP_DIR/gateway.log"
TEST_JSON="$TMP_DIR/test.json"
RECEIPTS_JSON="$TMP_DIR/receipts.json"
BUNDLE_JSON="$TMP_DIR/bundle.json"
RECOMPUTED_JSON="$TMP_DIR/recomputed.json"
EVIDENCE_JSON="$TMP_DIR/evidence.json"
VERIFIED_BUNDLE_JSON="$TMP_DIR/verified-bundle.json"
EXPORT_UNVERIFIED_JSON="$TMP_DIR/export-unverified.json"
EXPORT_VERIFIED_JSON="$TMP_DIR/export-verified.json"

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

log "generating gateway receipts through mayhem test"
(
  cd "$ROOT_DIR"
  cargo run -q -p mayhem-cli -- test \
    --gateway-url "$GATEWAY_URL" \
    --skip-peer-health \
    --skip-opencode \
    --json
) >"$TEST_JSON"

curl -fsS "$GATEWAY_URL/mayhem/receipts" >"$RECEIPTS_JSON"
node - "$RECEIPTS_JSON" <<'NODE'
const fs = require('node:fs');
const receipts = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
if (!Array.isArray(receipts.data) || receipts.data.length < 1) {
  console.error('error: gateway receipts response did not contain data[]');
  process.exit(1);
}
NODE

log "exporting epoch bundle with admin fee_bps=$FEE_BPS"
(
  cd "$ROOT_DIR"
  cargo run -q -p mayhem-cli -- receipts export \
    --epoch "$EPOCH" \
    --fee-bps "$FEE_BPS" \
    --receipts-file "$RECEIPTS_JSON" \
    --output "$BUNDLE_JSON" \
    --no-verify \
    --json
) >"$EXPORT_UNVERIFIED_JSON"

node "$ROOT_DIR/intercom/scripts/recompute-epoch-roots.mjs" "$BUNDLE_JSON" >"$RECOMPUTED_JSON"

node - "$RECOMPUTED_JSON" "$EVIDENCE_JSON" <<'NODE'
const fs = require('node:fs');
const [recomputedPath, evidencePath] = process.argv.slice(2);
const r = JSON.parse(fs.readFileSync(recomputedPath, 'utf8'));
const evidence = {
  [`ev/dep/${r.epoch}`]: {
    merkle_root: r.roots.dep,
    count: r.totals.dep_count,
    au_total: r.totals.dep_au,
  },
  [`ev/use/${r.epoch}`]: {
    merkle_root: r.roots.use,
    sessions: r.totals.use_count,
    au_total: r.totals.use_au,
    providers: r.totals.provider_count,
  },
  [`ev/earn/${r.epoch}`]: {
    merkle_root: r.roots.earn,
    provider_count: r.totals.provider_count,
    au_cum_total: r.totals.earn_au,
  },
  [`ev/fee/${r.epoch}`]: {
    merkle_root: r.roots.fee,
    au_fee_epoch: r.totals.fee_au,
    au_fee_cum: r.totals.fee_cum_au,
  },
};
fs.writeFileSync(evidencePath, `${JSON.stringify(evidence, null, 2)}\n`);
NODE

log "verifying receipt export against synthesized ev/* snapshot"
(
  cd "$ROOT_DIR"
  cargo run -q -p mayhem-cli -- receipts export \
    --epoch "$EPOCH" \
    --fee-bps "$FEE_BPS" \
    --receipts-file "$RECEIPTS_JSON" \
    --evidence-file "$EVIDENCE_JSON" \
    --output "$VERIFIED_BUNDLE_JSON" \
    --json
) >"$EXPORT_VERIFIED_JSON"

node - "$EXPORT_VERIFIED_JSON" "$RECOMPUTED_JSON" <<'NODE'
const fs = require('node:fs');
const [reportPath, recomputedPath] = process.argv.slice(2);
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
const recomputed = JSON.parse(fs.readFileSync(recomputedPath, 'utf8'));
function assert(condition, message) {
  if (!condition) {
    console.error(`error: ${message}`);
    process.exit(1);
  }
}
assert(report.verified === true, 'receipt export must verify against ev/* evidence');
assert(Array.isArray(report.checks) && report.checks.length === 16, 'expected 16 ev/* checks');
assert(report.checks.every((check) => check.ok === true), 'all ev/* checks must pass');
assert(report.bundle?.params?.fee_bps === recomputed.params?.fee_bps, 'fee_bps mismatch');
assert(Array.isArray(report.bundle?.receipts) && report.bundle.receipts.length >= 1, 'missing receipts');
console.log(JSON.stringify({
  ok: true,
  epoch: recomputed.epoch,
  fee_bps: recomputed.params.fee_bps,
  receipts: report.bundle.receipts.length,
  use_au: recomputed.totals.use_au,
  fee_au: recomputed.totals.fee_au,
  checks: report.checks.length,
  bundle_path: report.bundle_path,
}, null, 2));
NODE

log "local epoch receipt smoke passed"
if [[ "$KEEP_TMP" == "1" ]]; then
  log "kept temporary files in $TMP_DIR"
fi
