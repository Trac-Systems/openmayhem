#!/usr/bin/env bash
# OpenMayhem epoch settlement cadence (runs on the admin server via systemd timer).
#
# Canonical contract receipt metadata is the sole receipt source. A persisted
# quiet window lets both durable outboxes flush final checkpoints; it resets
# when the exact metadata identity changes and after a machine reboot. Payout maturity and
# retries are deliberately handled by the separate payout-worker timer.
set -euo pipefail

umask 077

RPC_URL="${MAYHEM_RPC_URL:-http://127.0.0.1:49223/v1}"
MAYHEM_BIN="${MAYHEM_BIN:-/opt/mayhem/source/target/release/mayhem}"
ADMIN_HOME="${MAYHEM_ADMIN_HOME:-/opt/mayhem/.mayhem-local/live-home}"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-/opt/mayhem/source}"
STATE_DIR="${MAYHEM_CADENCE_STATE_DIR:-/opt/mayhem/.mayhem-local/settlement}"
LOG_FILE="$STATE_DIR/cadence.log"
STAMP_FILE="$STATE_DIR/cadence.last-advance"
QUIET_STATE_FILE="$STATE_DIR/cadence.receipt-quiet.json"
QUIET_SECONDS="${MAYHEM_RECEIPT_QUIET_SECONDS:-30}"
BOOT_ID="${MAYHEM_CADENCE_BOOT_ID:-}"
if [[ -z "$BOOT_ID" && -r /proc/sys/kernel/random/boot_id ]]; then
    BOOT_ID="$(cat /proc/sys/kernel/random/boot_id)"
fi
BOOT_ID="${BOOT_ID:-unknown-boot}"

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

record_advance_stamp() {
    local advanced_at="${1:-}"
    positive_integer "$advanced_at" || {
        log "abort: canonical apply state did not return a positive advance timestamp"
        return 1
    }
    python3 - "$STAMP_FILE" "$advanced_at" <<'PY'
import os
import sys

path, stamp = sys.argv[1:]
tmp = f"{path}.tmp"
fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
try:
    with os.fdopen(fd, "w", closefd=False) as target:
        target.write(f"{stamp}\n")
        target.flush()
    os.fsync(fd)
finally:
    os.close(fd)
os.replace(tmp, path)
directory = os.open(os.path.dirname(path), os.O_RDONLY)
try:
    os.fsync(directory)
finally:
    os.close(directory)
PY
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

apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state&confirmed=true")"
updated_epoch="$(printf '%s' "$apply_state" | json_field value.updated_epoch)"
pending_epoch="$(printf '%s' "$apply_state" | json_field value.pending_epoch)"
epoch_seconds="$(printf '%s' "$apply_state" | json_field value.last_epoch_seconds)"
last_apply_hash="$(printf '%s' "$apply_state" | json_field value.last_apply_hash)"
last_receipt_commit_hash="$(printf '%s' "$apply_state" | json_field value.last_receipt_commit_hash)"
last_settlement_unix="$(printf '%s' "$apply_state" | json_field value.last_settlement_unix)"
if ! python3 - "$apply_state" <<'PY'
import json, sys
record = json.loads(sys.argv[1])
state = record.get("value") if isinstance(record, dict) else None
if (
    not isinstance(record, dict)
    or record.get("confirmed") is not True
    or record.get("key") not in (None, "epoch/apply/state")
    or not isinstance(state, dict)
):
    raise SystemExit("epoch/apply/state is not confirmed canonical state")
epoch = state.get("updated_epoch")
value = state.get("last_settlement_unix")
if epoch == 0:
    if value is not None:
        raise SystemExit("initial last_settlement_unix must be null")
elif value is not None and (
    not isinstance(value, int)
    or isinstance(value, bool)
    or value < 1
):
    raise SystemExit("last_settlement_unix must be a positive integer")
PY
then
    log "abort: epoch/apply/state.last_settlement_unix is not a positive canonical timestamp"
    exit 1
fi
epoch_seconds="${epoch_seconds:-3600}"
if ! non_negative_integer "$updated_epoch"; then
    log "abort: epoch/apply/state.updated_epoch is not a canonical non-negative integer"
    exit 1
fi
if ! positive_integer "$epoch_seconds"; then
    log "abort: epoch/apply/state.last_epoch_seconds is not a positive canonical integer"
    exit 1
fi
if [[ "$updated_epoch" == "0" ]]; then
    if [[ -n "$last_settlement_unix" ]]; then
        log "abort: epoch/apply/state.last_settlement_unix must be null before the first apply"
        exit 1
    fi
else
    if [[ ! "$last_apply_hash" =~ ^[0-9a-fA-F]{64}$ ]]; then
        log "abort: epoch/apply/state.last_apply_hash is not a canonical hash"
        exit 1
    fi
    last_apply_hash="$(printf '%s' "$last_apply_hash" | tr '[:upper:]' '[:lower:]')"
    if [[ -z "$last_settlement_unix" ]]; then
        last_settlement_unix="$(python3 - "$RPC_URL" "$updated_epoch" \
            "$last_apply_hash" "$last_receipt_commit_hash" <<'PY'
import json, re, sys, urllib.parse, urllib.request

rpc, epoch_text, apply_hash, receipt_commit_hash = sys.argv[1:]
epoch = int(epoch_text)

def state(key):
    query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
    with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
        record = json.load(response)
    if (
        not isinstance(record, dict)
        or record.get("confirmed") is not True
        or record.get("key") not in (None, key)
    ):
        raise SystemExit(f"canonical state {key} is not confirmed")
    return record.get("value")

def unix(value):
    return (
        isinstance(value, int)
        and not isinstance(value, bool)
        and value > 0
    )

seal = state(f"epoch/seal/{epoch}")
if seal is not None:
    if (
        not isinstance(seal, dict)
        or seal.get("type") != "epoch_empty_seal"
        or seal.get("epoch") != epoch
        or str(seal.get("seal_hash", "")).lower() != apply_hash
        or not unix(seal.get("at"))
    ):
        raise SystemExit("prior canonical empty-seal settlement identity is invalid")
    print(seal["at"])
    raise SystemExit(0)

commit = state(f"epoch/commit/{epoch}")
if (
    not isinstance(commit, dict)
    or commit.get("type") != "epoch_commit"
    or commit.get("epoch") != epoch
    or not unix(commit.get("at"))
    or not re.fullmatch(r"[0-9a-f]{64}", str(commit.get("commit_hash", "")).lower())
    or (
        receipt_commit_hash
        and str(commit.get("commit_hash", "")).lower() != receipt_commit_hash.lower()
    )
):
    raise SystemExit("prior canonical epoch commit settlement identity is invalid")
usage = state(f"ev/use/{epoch}")
if (
    not isinstance(usage, dict)
    or usage.get("type") != "usage_root"
    or usage.get("epoch") != epoch
    or usage.get("ts") != commit["at"]
    or usage.get("merkle_root") != (commit.get("roots") or {}).get("use")
):
    raise SystemExit("prior canonical epoch apply evidence is missing")
print(commit["at"])
PY
        )" || {
            log "abort: v16 canonical settlement timestamp could not be derived"
            exit 1
        }
        log "bootstrap: using confirmed epoch $updated_epoch settlement timestamp $last_settlement_unix"
    elif ! positive_integer "$last_settlement_unix"; then
        log "abort: epoch/apply/state.last_settlement_unix is not a positive canonical timestamp"
        exit 1
    fi
    positive_integer "$last_settlement_unix" || {
        log "abort: canonical settlement timestamp is invalid"
        exit 1
    }
fi
if ! positive_integer "$QUIET_SECONDS" || (( QUIET_SECONDS > 300 )); then
    log "abort: MAYHEM_RECEIPT_QUIET_SECONDS must be within 1..300"
    exit 1
fi
target=$((updated_epoch + 1))
if [[ -n "$pending_epoch" ]]; then
    if ! positive_integer "$pending_epoch" || [[ "$pending_epoch" != "$target" ]]; then
        log "abort: pending paged apply epoch $pending_epoch is not canonical next epoch $target"
        exit 1
    fi
    log "resume: pending bounded targeted apply for epoch $pending_epoch"
    if ! "$SOURCE_DIR/scripts/ops-settle-epoch.sh" "$pending_epoch"; then
        log "abort: pending targeted apply resume failed for epoch $pending_epoch"
        exit 1
    fi
    resumed_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state&confirmed=true")"
    resumed_epoch="$(printf '%s' "$resumed_state" | json_field value.updated_epoch)"
    resumed_settlement_unix="$(printf '%s' "$resumed_state" | json_field value.last_settlement_unix)"
    if [[ "$resumed_epoch" != "$pending_epoch" ]]; then
        log "abort: targeted apply resume returned but updated_epoch is $resumed_epoch"
        exit 1
    fi
    if ! positive_integer "$resumed_settlement_unix" || \
        (( resumed_settlement_unix < last_settlement_unix )); then
        log "abort: targeted apply resume returned an invalid canonical settlement timestamp"
        exit 1
    fi
    record_advance_stamp "$resumed_settlement_unix" || exit 1
    log "resumed and finalized non-empty epoch $pending_epoch"
    exit 0
fi
now="$(date +%s)"
if ! positive_integer "$now"; then
    log "abort: system clock did not return a positive canonical timestamp"
    exit 1
fi

# Canonical contract time is authoritative. The local stamp is only a durable
# regression detector and audit mirror of the last confirmed apply timestamp.
if [[ -f "$STAMP_FILE" ]]; then
    last_advance="$(cat "$STAMP_FILE" 2>/dev/null || echo 0)"
    if ! non_negative_integer "$last_advance"; then
        log "abort: cadence advance stamp is not a canonical non-negative timestamp"
        exit 1
    fi
    if (( last_advance > last_settlement_unix )); then
        log "abort: local cadence stamp is ahead of canonical apply time"
        exit 1
    fi
fi
if (( now < last_settlement_unix )); then
    log "abort: canonical settlement timestamp $last_settlement_unix is in the future"
    exit 1
fi
if (( now < last_settlement_unix + epoch_seconds )); then
    log "skip: epoch $target window has not elapsed (canonical settlement $last_settlement_unix)"
    exit 0
fi

metadata_summary="$(python3 - "$RPC_URL" "$target" <<'PY'
import hashlib, json, sys, urllib.parse, urllib.request
rpc, epoch_text = sys.argv[1:]
epoch = int(epoch_text)

def state(key):
    query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
    with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
        record = json.load(response)
    if (
        not isinstance(record, dict)
        or record.get("confirmed") is not True
        or record.get("key") not in (None, key)
    ):
        raise SystemExit(f"canonical state {key} is not confirmed")
    return record.get("value")

metadata = state(f"receipt/epoch/{epoch}/index")
empty = metadata is None
if empty:
    metadata = {
        "epoch": epoch,
        "count": 0,
        "page_count": 0,
        "revision": 0,
        "updated_at": None,
    }
if (
    not isinstance(metadata, dict)
    or metadata.get("epoch") != epoch
    or not isinstance(metadata.get("count"), int)
    or isinstance(metadata.get("count"), bool)
    or metadata["count"] < 0
    or not isinstance(metadata.get("page_count"), int)
    or isinstance(metadata.get("page_count"), bool)
    or metadata["page_count"] < 0
    or not isinstance(metadata.get("revision"), int)
    or isinstance(metadata.get("revision"), bool)
    or metadata["revision"] < 0
):
    raise SystemExit("canonical receipt index is malformed")
if not empty and (
    metadata.get("type") != "canonical_receipt_epoch_index"
    or not isinstance(metadata.get("page_size"), int)
    or isinstance(metadata.get("page_size"), bool)
    or metadata["page_size"] < 1
    or metadata["page_size"] > 1000
    or not isinstance(metadata.get("updated_at"), str)
    or not metadata["updated_at"]
):
    raise SystemExit("canonical receipt index identity is malformed")
count = metadata["count"]
page_count = metadata["page_count"]
if (count == 0) != (page_count == 0) or page_count > count:
    raise SystemExit("canonical receipt index count/page_count mismatch")
if empty and (count != 0 or page_count != 0 or metadata["revision"] != 0):
    raise SystemExit("canonical null receipt index is not empty")
if not empty and page_count != ((count + metadata["page_size"] - 1) // metadata["page_size"]):
    raise SystemExit("canonical receipt index page_count is inconsistent")
metadata_hash = hashlib.sha256(
    json.dumps(metadata, sort_keys=True, separators=(",", ":")).encode()
).hexdigest()
print(f"{count} {metadata_hash}")
PY
)" || {
    log "abort: canonical receipt metadata for epoch $target failed validation"
    exit 1
}
receipt_count="${metadata_summary%% *}"
receipt_metadata_hash="${metadata_summary##* }"
if ! non_negative_integer "$receipt_count" || [[ ! "$receipt_metadata_hash" =~ ^[0-9a-f]{64}$ ]]; then
    log "abort: canonical receipt metadata summary is invalid"
    exit 1
fi

quiet_status="$(python3 - "$QUIET_STATE_FILE" "$target" "$receipt_count" \
    "$receipt_metadata_hash" "$now" "$QUIET_SECONDS" "$BOOT_ID" <<'PY'
import json, os, sys
path, epoch, count, metadata_hash, now, quiet, boot_id = sys.argv[1:]
expected = {
    "epoch": int(epoch),
    "count": int(count),
    "metadata_hash": metadata_hash,
    "boot_id": boot_id,
}
now = int(now)
quiet = int(quiet)
prior = None
try:
    prior = json.load(open(path))
except (FileNotFoundError, json.JSONDecodeError, OSError):
    pass
same = isinstance(prior, dict) and all(prior.get(key) == value for key, value in expected.items())
if not same:
    state = {**expected, "observed_at": now}
    tmp = f"{path}.tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", closefd=False) as target:
            json.dump(state, target, sort_keys=True)
            target.write("\n")
            target.flush()
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(tmp, path)
    directory = os.open(os.path.dirname(path), os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    print("wait")
else:
    observed_at = prior.get("observed_at")
    if not isinstance(observed_at, int) or observed_at < 0:
        raise SystemExit("persisted receipt quiet state is malformed")
    print("ready" if now >= observed_at + quiet else "wait")
PY
)" || {
    log "abort: canonical receipt quiet-window state is invalid"
    exit 1
}
if [[ "$quiet_status" != "ready" ]]; then
    log "skip: epoch $target receipt set count=$receipt_count metadata=$receipt_metadata_hash is awaiting ${QUIET_SECONDS}s quiet"
    exit 0
fi

if [[ "$receipt_count" != "0" ]]; then
    log "finalize: canonical epoch $target has $receipt_count receipt(s); invoking exact-key finalizer"
    if ! "$SOURCE_DIR/scripts/ops-settle-epoch.sh" "$target"; then
        log "abort: canonical receipt finalizer failed for epoch $target"
        exit 1
    fi
    finalized_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
    finalized_epoch="$(printf '%s' "$finalized_state" | json_field value.updated_epoch)"
    finalized_settlement_unix="$(
        printf '%s' "$finalized_state" | json_field value.last_settlement_unix
    )"
    if [[ "$finalized_epoch" != "$target" ]]; then
        log "abort: canonical receipt finalizer returned but updated_epoch is $finalized_epoch"
        exit 1
    fi
    if ! positive_integer "$finalized_settlement_unix" || \
        (( finalized_settlement_unix < last_settlement_unix )); then
        log "abort: canonical receipt finalizer returned an invalid settlement timestamp"
        exit 1
    fi
    record_advance_stamp "$finalized_settlement_unix" || exit 1
    log "finalized non-empty epoch $target from contract-recorded receipt heads"
    exit 0
fi

reason="cadence: canonical receipt metadata is empty for epoch $target"
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
after_settlement_unix="$(printf '%s' "$after_state" | json_field value.last_settlement_unix)"
if [[ "$after_epoch" != "$target" ]]; then
    log "abort: sealed epoch $target but updated_epoch is $after_epoch"
    exit 1
fi
if ! positive_integer "$after_settlement_unix" || \
    (( after_settlement_unix < last_settlement_unix )); then
    log "abort: empty epoch seal returned an invalid canonical settlement timestamp"
    exit 1
fi
record_advance_stamp "$after_settlement_unix" || exit 1
log "sealed epoch $target empty; billing epoch is now $((target + 1))"
