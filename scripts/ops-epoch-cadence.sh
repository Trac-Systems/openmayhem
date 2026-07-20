#!/usr/bin/env bash
# OpenMayhem epoch settlement cadence (runs on the admin server via systemd timer).
#
# Purpose: spend-reservation holds (`hold/<rail>/<user>/<epoch>`) pin user
# balance until the billing epoch advances, and the epoch only advances when
# the admin settles it. This script bounds stale-hold lifetime to one cadence
# interval by sealing the active billing epoch empty — but ONLY when it is
# provably safe to do so:
#   - no pending paged epoch apply,
#   - at least one epoch window elapsed since this script last advanced,
#   - the TAP gateway reports no active sessions,
#   - the TAP gateway retains no receipts for the target epoch.
# An epoch with retained receipts is NEVER sealed away. It is passed only to the
# canonical retained-receipt finalizer, which gates finalization and payout.
set -euo pipefail

umask 077

RPC_URL="${MAYHEM_RPC_URL:-http://127.0.0.1:49223/v1}"
GATEWAY_URL="${MAYHEM_GATEWAY_URL:-http://127.0.0.1:11445}"
MAYHEM_BIN="${MAYHEM_BIN:-/opt/mayhem/source/target/release/mayhem}"
ADMIN_HOME="${MAYHEM_ADMIN_HOME:-/opt/mayhem/.mayhem-local/live-home}"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-/opt/mayhem/source}"
STATE_DIR="${MAYHEM_CADENCE_STATE_DIR:-/opt/mayhem/.mayhem-local/settlement}"
LOG_FILE="$STATE_DIR/cadence.log"
STAMP_FILE="$STATE_DIR/cadence.last-advance"

mkdir -p "$STATE_DIR"
# Refuse to act before we can record what we did: an unwritable state dir must
# never lead to a seal with no stamp/log (that breaks the once-per-window guard).
touch "$LOG_FILE" "$STAMP_FILE.probe" && rm -f "$STAMP_FILE.probe" || {
    echo "abort: settlement state dir $STATE_DIR is not writable" >&2
    exit 1
}

log() {
    echo "$(date -u +%FT%TZ) $*" >>"$LOG_FILE"
    echo "$*"
}

json_field() {
    python3 -c '
import json, sys
value = json.loads(sys.stdin.read())
for part in sys.argv[1].split("."):
    if value is None:
        break
    value = value.get(part) if isinstance(value, dict) else None
print("" if value is None else value)
' "$1"
}

positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]] && \
        python3 -c 'import sys; raise SystemExit(0 if int(sys.argv[1]) <= 9007199254740991 else 1)' "$1"
}

non_negative_integer() {
    [[ "$1" =~ ^(0|[1-9][0-9]*)$ ]] && \
        python3 -c 'import sys; raise SystemExit(0 if int(sys.argv[1]) <= 9007199254740991 else 1)' "$1"
}

command -v flock >/dev/null 2>&1 || {
    log "abort: flock is required for epoch/payout serialization"
    exit 1
}
exec 9>"$STATE_DIR/payout.lock"
if ! flock -n 9; then
    log "skip: another payout/finalization worker holds $STATE_DIR/payout.lock"
    exit 0
fi
export MAYHEM_PAYOUT_LOCK_HELD=1

apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
updated_epoch="$(printf '%s' "$apply_state" | json_field value.updated_epoch)"
pending_epoch="$(printf '%s' "$apply_state" | json_field value.pending_epoch)"
epoch_seconds="$(printf '%s' "$apply_state" | json_field value.last_epoch_seconds)"
epoch_seconds="${epoch_seconds:-3600}"
if ! non_negative_integer "$updated_epoch"; then
    log "abort: epoch/apply/state.updated_epoch is not a canonical non-negative integer"
    exit 1
fi
if ! positive_integer "$epoch_seconds"; then
    log "abort: epoch/apply/state.last_epoch_seconds is not a positive canonical integer"
    exit 1
fi
if [[ -n "$pending_epoch" ]]; then
    log "skip: pending paged epoch apply for epoch $pending_epoch"
    exit 0
fi
if [[ "$updated_epoch" != "0" ]]; then
    apply_hash="$(printf '%s' "$apply_state" | json_field value.last_apply_hash)"
    if [[ ! "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]]; then
        log "abort: current applied epoch has no canonical apply hash"
        exit 1
    fi
    apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"
    if [[ -f "$STATE_DIR/epochs/epoch-$updated_epoch/epoch-bundle.json" ]]; then
        if ! "$SOURCE_DIR/scripts/ops-settle-epoch.sh" "$updated_epoch"; then
            log "abort: retained-artifact finalization recovery failed for current epoch $updated_epoch"
            exit 1
        fi
    else
        if ! "$SOURCE_DIR/scripts/ops-payout-settle.sh" "$updated_epoch"; then
            log "abort: payout reconciliation failed for current epoch $updated_epoch"
            exit 1
        fi
    fi
    payout_dir="$STATE_DIR/payout/epoch-$updated_epoch-$apply_hash"
    if [[ ! -f "$payout_dir/complete" ]]; then
        log "skip: current epoch $updated_epoch payouts remain incomplete"
        exit 0
    fi
fi
target=$((updated_epoch + 1))
now="$(date +%s)"
if ! positive_integer "$now"; then
    log "abort: system clock did not return a positive canonical timestamp"
    exit 1
fi

# Epoch numbers on this deployment are settlement-paced, not wall-clock-paced,
# so the contract's own window check (at >= epoch * epoch_seconds) is vacuous.
# Enforce the cadence locally: only advance once per epoch window.
if [[ -f "$STAMP_FILE" ]]; then
    last_advance="$(cat "$STAMP_FILE" 2>/dev/null || echo 0)"
    if ! non_negative_integer "$last_advance"; then
        log "abort: cadence advance stamp is not a canonical non-negative timestamp"
        exit 1
    fi
    if (( now < last_advance + epoch_seconds )); then
        log "skip: epoch $target window has not elapsed (last advance $last_advance)"
        exit 0
    fi
fi

status_json="$(curl -sf -m 10 "$GATEWAY_URL/mayhem/status" || echo '')"
if [[ -z "$status_json" ]]; then
    log "skip: gateway status unreachable; refusing to seal blind"
    exit 0
fi
sessions_active="$(printf '%s' "$status_json" | json_field sessions_active)"
if ! non_negative_integer "$sessions_active"; then
    log "skip: gateway sessions_active is missing or invalid"
    exit 0
fi
if [[ "$sessions_active" != "0" ]]; then
    log "skip: gateway reports sessions_active=$sessions_active"
    exit 0
fi

receipts_json="$(curl -sf -m 10 "$GATEWAY_URL/mayhem/receipts" || echo '')"
if [[ -z "$receipts_json" ]]; then
    log "skip: gateway receipts unreachable; refusing to seal blind"
    exit 0
fi
# Stored receipts carry no epoch field (the epoch lives in the hold record),
# so the only safe rule is: ANY retained receipt blocks sealing. Retained
# receipts always belong to a not-yet-settled epoch; sealing would forfeit
# provider earnings. /mayhem/receipts returns {"object":"list","data":[...]}.
retained_receipts="$(printf '%s' "$receipts_json" | python3 -c '
import json, sys
data = json.loads(sys.stdin.read())
receipts = data if isinstance(data, list) else data.get("data", data.get("receipts", []))
print(len(receipts))
')"
if ! non_negative_integer "$retained_receipts"; then
    log "abort: gateway retained receipt count is not a canonical non-negative integer"
    exit 1
fi
if [[ "$retained_receipts" != "0" ]]; then
    prior_receipts="$STATE_DIR/epochs/epoch-$updated_epoch/gateway-receipts.json"
    if [[ "$updated_epoch" != "0" && -f "$prior_receipts" ]]; then
        overlap="$(printf '%s' "$receipts_json" | python3 -c '
import hashlib, json, sys
prior_path = sys.argv[1]
current_response = json.loads(sys.stdin.read())
current = current_response if isinstance(current_response, list) else current_response.get("data")
prior = json.load(open(prior_path))
if not isinstance(current, list) or not isinstance(prior, list):
    raise SystemExit("gateway receipt overlap evidence must be lists")
def fingerprint(value):
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()
prior_hashes = {fingerprint(item) for item in prior}
print(sum(1 for item in current if fingerprint(item) in prior_hashes))
' "$prior_receipts")"
        if ! non_negative_integer "$overlap"; then
            log "abort: finalized gateway receipt overlap count is invalid"
            exit 1
        fi
        if [[ "$overlap" != "0" ]]; then
            log "skip: gateway still retains $overlap receipt(s) already bound to epoch $updated_epoch"
            exit 0
        fi
    fi
    log "finalize: gateway retains $retained_receipts receipt(s); invoking canonical finalizer for epoch $target"
    if ! "$SOURCE_DIR/scripts/ops-settle-epoch.sh" "$target"; then
        log "abort: canonical retained-receipt finalizer failed for epoch $target"
        exit 1
    fi
    finalized_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
    finalized_epoch="$(printf '%s' "$finalized_state" | json_field value.updated_epoch)"
    if [[ "$finalized_epoch" != "$target" ]]; then
        log "abort: retained-receipt finalizer returned but updated_epoch is $finalized_epoch"
        exit 1
    fi
    log "finalized non-empty epoch $target through the canonical gated finalizer"
    exit 0
fi

reason="cadence: no retained receipts for epoch $target"
seal_args=(
    admin epoch-seal-empty
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --epoch "$target"
    --at "$now"
    --reason "$reason"
    --submit --json
)
if ! sim_out="$("$MAYHEM_BIN" "${seal_args[@]}" --sim 2>&1)"; then
    log "abort: seal-empty sim for epoch $target failed: $sim_out"
    exit 1
fi
if ! submit_out="$("$MAYHEM_BIN" "${seal_args[@]}" 2>&1)"; then
    log "abort: seal-empty submit for epoch $target failed: $submit_out"
    exit 1
fi

after_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
after_epoch="$(printf '%s' "$after_state" | json_field value.updated_epoch)"
if [[ "$after_epoch" != "$target" ]]; then
    log "abort: sealed epoch $target but updated_epoch is $after_epoch"
    exit 1
fi
echo "$now" >"$STAMP_FILE"
log "sealed epoch $target empty; billing epoch is now $((target + 1))"
