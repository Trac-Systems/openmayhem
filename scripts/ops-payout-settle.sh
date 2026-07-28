#!/usr/bin/env bash
# Reconcile all operator-owned payout rails for one canonically applied epoch.
# Fiat and TNK execute from ledger liabilities through the mayhem CLI. TAP work
# is derived only from the retained canonical finalized-epoch snapshot and
# handed to the existing simulate-first TAP settlement worker.
set -euo pipefail

umask 077

RPC_URL="${MAYHEM_RPC_URL:-http://127.0.0.1:49223/v1}"
MAYHEM_BIN="${MAYHEM_BIN:-/opt/mayhem/source/target/release/mayhem}"
ADMIN_HOME="${MAYHEM_ADMIN_HOME:-/opt/mayhem/.mayhem-local/live-home}"
ADMIN_STORE="${MAYHEM_ADMIN_STORE:-mayhem-canonical-admin}"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-/opt/mayhem/source}"
STATE_DIR="${MAYHEM_CADENCE_STATE_DIR:-/opt/mayhem/.mayhem-local/settlement}"
TAP_SPOOL="${MAYHEM_TAP_SETTLEMENT_SPOOL:-$STATE_DIR/tap}"
FIAT_ENABLED="${MAYHEM_FIAT_SETTLEMENT_ENABLED:-1}"
TNK_ENABLED="${MAYHEM_TNK_SETTLEMENT_ENABLED:-1}"
TAP_ENABLED="${MAYHEM_TAP_SETTLEMENT_ENABLED:-1}"
MAX_ATTEMPTS="${MAYHEM_PAYOUT_MAX_ATTEMPTS:-8}"
RETRY_BACKOFF_SECONDS="${MAYHEM_PAYOUT_RETRY_BACKOFF_SECONDS:-300}"
FIAT_OPERATOR_ACCOUNT="${MAYHEM_FIAT_OPERATOR_ACCOUNT:-platform_balance}"
FIAT_STRIPE_ENV_FILE="${MAYHEM_FIAT_STRIPE_ENV_FILE:-}"
FIAT_TRANSFER_MAX_ATTEMPTS="${MAYHEM_FIAT_TRANSFER_MAX_ATTEMPTS:-4}"
FIAT_TRANSFER_RETRY_MS="${MAYHEM_FIAT_TRANSFER_RETRY_MS:-500}"
TNK_TRANSFER_TIMEOUT_SECONDS="${MAYHEM_TNK_TRANSFER_TIMEOUT_SECONDS:-180}"
TNK_TRANSFER_MAX_RETRIES="${MAYHEM_TNK_TRANSFER_MAX_RETRIES:-3}"
TAP_CONTRACT_WAIT_ATTEMPTS="${MAYHEM_TAP_CONTRACT_WAIT_ATTEMPTS:-120}"
TAP_CONTRACT_WAIT_SECONDS="${MAYHEM_TAP_CONTRACT_WAIT_SECONDS:-1}"

json_field() {
  python3 -c '
import json, sys
value = json.loads(sys.stdin.read())
for part in sys.argv[1].split("."):
    if value is None:
        break
    value = value.get(part) if isinstance(value, dict) else None
if isinstance(value, bool):
    print(str(value).lower())
elif value is None:
    print("")
else:
    print(value)
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

for value in \
  "$MAX_ATTEMPTS" \
  "$RETRY_BACKOFF_SECONDS" \
  "$FIAT_TRANSFER_MAX_ATTEMPTS" \
  "$FIAT_TRANSFER_RETRY_MS" \
  "$TNK_TRANSFER_TIMEOUT_SECONDS" \
  "$TNK_TRANSFER_MAX_RETRIES" \
  "$TAP_CONTRACT_WAIT_ATTEMPTS" \
  "$TAP_CONTRACT_WAIT_SECONDS"; do
  positive_integer "$value" || {
    echo "abort: payout retry and timeout settings must be positive integers" >&2
    exit 1
  }
done

validate_execution_context() {
  if [[ "${MAYHEM_PAYOUT_TEST_MODE:-0}" == "1" ]]; then
    local test_root="${MAYHEM_PAYOUT_TEST_ROOT:-}"
    [[ -n "$test_root" && -d "$test_root" ]] || {
      echo "abort: payout test mode requires an existing MAYHEM_PAYOUT_TEST_ROOT" >&2
      return 1
    }
    for path in "$MAYHEM_BIN" "$ADMIN_HOME" "$STATE_DIR" "$TAP_SPOOL"; do
      [[ "$path" == "$test_root"/* ]] || {
        echo "abort: payout test mode path escapes MAYHEM_PAYOUT_TEST_ROOT: $path" >&2
        return 1
      }
    done
    [[ "$RPC_URL" == "http://mock.invalid/v1" ]] || {
      echo "abort: payout test mode requires the isolated mock RPC URL" >&2
      return 1
    }
    for key in \
      MAYHEM_STRIPE_SECRET_KEY \
      STRIPE_SECRET_KEY \
      MAYHEM_FIAT_STRIPE_ENV_FILE \
      MAYHEM_ADMIN_WALLET_PASSWORD \
      MAYHEM_WALLET_PASSWORD \
      MAYHEM_TAP_ROLLER_PRIVATE_KEY \
      TAP_ROLLER_PRIVATE_KEY \
      MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY \
      TAP_GOVERNANCE_PRIVATE_KEY \
      MAYHEM_TAP_ETH_RPC \
      MAYHEM_TAP_ETH_RPC_FALLBACKS \
      MAYHEM_TNK_TREASURY_KEYPAIR_PATH \
      MAYHEM_TNK_WALLET_PASSWORD; do
      [[ -z "${!key:-}" ]] || {
        echo "abort: payout test mode refuses inherited credential $key" >&2
        return 1
      }
    done
    return 0
  fi

  [[ "${MAYHEM_NETWORK:-}" == "mainnet" && "${MAYHEM_MSB_NETWORK:-}" == "mainnet" ]] || {
    echo "abort: Mayhem and MSB networks are not canonical mainnet" >&2
    return 1
  }
  [[ "${MSB_BOOTSTRAP:-}" == "acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685" ]] || {
    echo "abort: MSB_BOOTSTRAP is not the official mainnet bootstrap" >&2
    return 1
  }
  [[ "${MSB_CHANNEL:-}" == "0000trac0network0msb0mainnet0000" ]] || {
    echo "abort: MSB_CHANNEL is not the official mainnet channel" >&2
    return 1
  }
  [[ "${MAYHEM_STRIPE_MODE:-}" == "live" ]] || {
    echo "abort: MAYHEM_STRIPE_MODE is not live" >&2
    return 1
  }
  [[ "${MAYHEM_TAP_ETH_CHAIN_ID:-}" == "1" ]] || {
    echo "abort: MAYHEM_TAP_ETH_CHAIN_ID is not canonical Ethereum mainnet" >&2
    return 1
  }
  [[ "${MAYHEM_STRIPE_API_BASE_URL:-}" == "https://api.stripe.com" ]] || {
    echo "abort: MAYHEM_STRIPE_API_BASE_URL is not the canonical live endpoint" >&2
    return 1
  }
  [[ "${MAYHEM_PAYGATE_CONTRACT_DRY_RUN:-}" == "0" ]] || {
    echo "abort: MAYHEM_PAYGATE_CONTRACT_DRY_RUN must be 0 for live payout" >&2
    return 1
  }
  [[ "${MAYHEM_PAYGATE_STRIPE_ENABLED:-}" == "1" ]] || {
    echo "abort: MAYHEM_PAYGATE_STRIPE_ENABLED must be 1 for live payout" >&2
    return 1
  }
  [[ "$FIAT_ENABLED" == "1" && "$TAP_ENABLED" == "1" && "$TNK_ENABLED" == "1" ]] || {
    echo "abort: all automatic payout rails must be enabled on mainnet" >&2
    return 1
  }
  [[ "${MAYHEM_STRIPE_SECRET_KEY:-}" == sk_live_* ]] || {
    echo "abort: MAYHEM_STRIPE_SECRET_KEY is not a live Stripe key" >&2
    return 1
  }
  [[ "${MAYHEM_TNK_TREASURY_ADDRESS:-}" == "trac1f3w8ja3qxcnmzzmxxt8m0ystdf683sy5arnhxvz0h7a8ydd0kqwq3lcgdh" ]] || {
    echo "abort: MAYHEM_TNK_TREASURY_ADDRESS is not the canonical mainnet treasury" >&2
    return 1
  }
  [[ -n "${MAYHEM_TNK_OPERATOR_ADDRESS:-}" && -f "${MAYHEM_TNK_TREASURY_KEYPAIR_PATH:-}" ]] || {
    echo "abort: live TNK operator or treasury keypair setting is missing" >&2
    return 1
  }
  [[ "${MAYHEM_TAP_POOL_ADDRESS:-}" == "0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28" ]] || {
    echo "abort: MAYHEM_TAP_POOL_ADDRESS is not the canonical mainnet pool" >&2
    return 1
  }
  [[ "${MAYHEM_TAP_TOKEN_ADDR:-}" == "0x5e7F6e008C6d9D7AD4c7EB75Bd4ce62864cc7454" ]] || {
    echo "abort: MAYHEM_TAP_TOKEN_ADDR is not the canonical mainnet token" >&2
    return 1
  }
  [[ "${MAYHEM_TAP_OPERATOR_ADDRESS:-}" =~ ^0x[0-9a-fA-F]{40}$ ]] || {
    echo "abort: MAYHEM_TAP_OPERATOR_ADDRESS is invalid" >&2
    return 1
  }
  [[ "${MAYHEM_TAP_ROLLER_PRIVATE_KEY:-}" =~ ^0x[0-9a-fA-F]{64}$ && \
     "${MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY:-}" =~ ^0x[0-9a-fA-F]{64}$ && \
     "${MAYHEM_TAP_ROLLER_PRIVATE_KEY:-}" != "${MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY:-}" ]] || {
    echo "abort: live TAP owner and governance signers are missing, invalid, or identical" >&2
    return 1
  }
  [[ -f "$SOURCE_DIR/scripts/mainnet-proof.mjs" ]] || {
    echo "abort: canonical mainnet proof helper is missing" >&2
    return 1
  }
  command -v node >/dev/null 2>&1 || {
    echo "abort: node is required for canonical mainnet proof" >&2
    return 1
  }
  local proof_tmp="$STATE_DIR/payout/mainnet-proof.json.tmp"
  if ! node "$SOURCE_DIR/scripts/mainnet-proof.mjs" \
    --peer-rpc "$RPC_URL" --timeout-seconds 0 --json >"$proof_tmp"; then
    rm -f "$proof_tmp"
    echo "abort: canonical mainnet proof failed; refusing payout execution" >&2
    return 1
  fi
  mv "$proof_tmp" "$STATE_DIR/payout/mainnet-proof.json"
}

for value in "$FIAT_ENABLED" "$TNK_ENABLED" "$TAP_ENABLED"; do
  [[ "$value" == "0" || "$value" == "1" ]] || {
    echo "abort: payout rail enable settings must be 0 or 1" >&2
    exit 1
  }
done

mkdir -p \
  "$STATE_DIR/payout" \
  "$TAP_SPOOL/ready" \
  "$TAP_SPOOL/working" \
  "$TAP_SPOOL/processed" \
  "$TAP_SPOOL/failed"

if [[ "${MAYHEM_PAYOUT_LOCK_HELD:-0}" != "1" ]]; then
  command -v flock >/dev/null 2>&1 || {
    echo "abort: flock is required for payout worker serialization" >&2
    exit 1
  }
  exec 9>"$STATE_DIR/payout.lock"
  if ! flock -n 9; then
    echo "skip: another payout/finalization worker holds $STATE_DIR/payout.lock"
    exit 0
  fi
  export MAYHEM_PAYOUT_LOCK_HELD=1
fi

canonical_apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state&confirmed=true")"
canonical_applied_epoch="$(
  printf '%s' "$canonical_apply_state" | json_field value.updated_epoch
)"
canonical_pending_epoch="$(
  printf '%s' "$canonical_apply_state" | json_field value.pending_epoch
)"
canonical_apply_hash="$(
  printf '%s' "$canonical_apply_state" | json_field value.last_apply_hash
)"
canonical_settlement_unix="$(
  printf '%s' "$canonical_apply_state" | json_field value.last_settlement_unix
)"
if ! python3 - "$canonical_apply_state" <<'PY'
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
if (
    not isinstance(epoch, int)
    or isinstance(epoch, bool)
    or epoch < 0
    or (
        epoch > 0
        and (
            not isinstance(value, int)
            or isinstance(value, bool)
            or value < 1
        )
    )
):
    raise SystemExit("epoch/apply/state settlement timestamp is invalid")
PY
then
  echo "abort: canonical epoch/apply/state.last_settlement_unix is invalid" >&2
  exit 1
fi

[[ "$canonical_applied_epoch" =~ ^[0-9]+$ ]] || {
  echo "abort: canonical epoch/apply/state.updated_epoch is missing or invalid" >&2
  exit 1
}
if [[ "$canonical_applied_epoch" == "0" ]]; then
  echo "skip: no epoch has been finalized yet"
  exit 0
fi
if [[ -n "$canonical_pending_epoch" ]]; then
  echo "skip: epoch $canonical_pending_epoch still has a pending paged apply"
  exit 0
fi
[[ "$canonical_apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
  echo "abort: canonical epoch/apply/state.last_apply_hash is not a 32-byte hash" >&2
  exit 1
}
canonical_apply_hash="$(
  printf '%s' "$canonical_apply_hash" | tr '[:upper:]' '[:lower:]'
)"
positive_integer "$canonical_settlement_unix" || {
  echo "abort: canonical epoch/apply/state.last_settlement_unix is not a positive timestamp" >&2
  exit 1
}

resume_work_dir="${MAYHEM_PAYOUT_RESUME_WORK_DIR:-}"
if [[ -n "$resume_work_dir" ]]; then
  [[ "${MAYHEM_PAYOUT_RESUME_PRIOR:-0}" == "1" ]] || {
    echo "abort: prior payout work directory requires internal resume mode" >&2
    exit 1
  }
  python3 - "$resume_work_dir" "$STATE_DIR/payout" <<'PY'
import os, re, sys
candidate, root = map(os.path.abspath, sys.argv[1:])
if os.path.dirname(candidate) != root:
    raise SystemExit("prior payout work directory escapes the canonical payout state root")
if not re.fullmatch(r"epoch-[1-9][0-9]*-[0-9a-f]{64}", os.path.basename(candidate)):
    raise SystemExit("prior payout work directory has a non-canonical name")
if not os.path.isdir(candidate) or os.path.islink(candidate):
    raise SystemExit("prior payout work directory is missing, not a directory, or a symlink")
PY
  work_name="${resume_work_dir##*/}"
  applied_epoch="${work_name#epoch-}"
  applied_epoch="${applied_epoch%%-*}"
  apply_hash="${work_name##*-}"
  work_dir="$resume_work_dir"
  epoch_dir="$STATE_DIR/epochs/epoch-$applied_epoch"
  requested_epoch="${1:-$applied_epoch}"
  [[ "$requested_epoch" == "$applied_epoch" ]] || {
    echo "abort: internal payout resume epoch does not match its retained work directory" >&2
    exit 1
  }
  apply_state_file="$work_dir/apply-state.json"
  [[ -f "$apply_state_file" ]] || {
    echo "abort: prior payout work lacks its frozen canonical apply state" >&2
    exit 1
  }
  apply_state="$(cat "$apply_state_file")"
  python3 - "$apply_state_file" "$applied_epoch" "$apply_hash" <<'PY'
import json, sys
state = json.load(open(sys.argv[1])).get("value") or {}
if (
    state.get("updated_epoch") != int(sys.argv[2])
    or str(state.get("last_apply_hash", "")).lower() != sys.argv[3]
    or not isinstance(state.get("last_settlement_unix"), int)
    or isinstance(state.get("last_settlement_unix"), bool)
    or state.get("last_settlement_unix") < 1
):
    raise SystemExit("prior payout work does not match its frozen canonical apply state")
PY
  canonical_settlement_unix="$(
    printf '%s' "$apply_state" | json_field value.last_settlement_unix
  )"
else
  applied_epoch="$canonical_applied_epoch"
  apply_hash="$canonical_apply_hash"
  requested_epoch="${1:-$applied_epoch}"
  [[ "$requested_epoch" == "$applied_epoch" ]] || {
    echo "abort: payout epoch $requested_epoch is not the current applied epoch $applied_epoch" >&2
    exit 1
  }
  work_dir="$STATE_DIR/payout/epoch-$applied_epoch-$apply_hash"
  epoch_dir="$STATE_DIR/epochs/epoch-$applied_epoch"
  mkdir -p "$work_dir"
  apply_state="$canonical_apply_state"
  printf '%s\n' "$apply_state" >"$work_dir/apply-state.json.tmp"
  mv "$work_dir/apply-state.json.tmp" "$work_dir/apply-state.json"
fi

[[ "$requested_epoch" =~ ^[1-9][0-9]*$ ]] || {
  echo "abort: requested payout epoch must be a positive canonical integer" >&2
  exit 1
}
[[ "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
  echo "abort: payout work apply hash is not a 32-byte hash" >&2
  exit 1
}
apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"

validate_execution_context

validate_epoch_artifact() {
  local bundle="$epoch_dir/epoch-bundle.json"
  local recomputed="$epoch_dir/epoch-recomputed.json"
  local artifact="$epoch_dir/epoch-artifact.json"
  local canonical_receipts="$epoch_dir/canonical-receipts.json"
  [[ -f "$bundle" ]] || return 0
  [[ -f "$recomputed" && -f "$artifact" && -f "$canonical_receipts" ]] || {
    echo "abort: retained epoch bundle is missing recomputed, canonical receipt, or apply-bound evidence" >&2
    return 1
  }
  local epoch_commit apply_anchor
  epoch_commit="$(
    curl -sf -m 10 \
      "$RPC_URL/state?key=epoch/commit/$applied_epoch&confirmed=true"
  )"
  apply_anchor="$(
    curl -sf -m 10 \
      "$RPC_URL/state?key=epoch/apply-anchor/$applied_epoch&confirmed=true"
  )"
  python3 - "$bundle" "$recomputed" "$canonical_receipts" "$artifact" \
    "$applied_epoch" "$apply_hash" "$apply_state" "$epoch_commit" "$apply_anchor" <<'PY'
import hashlib, json, re, sys
(
    bundle_path,
    recomputed_path,
    receipts_path,
    artifact_path,
    expected_epoch,
    expected_hash,
    apply_state_raw,
    commit_raw,
    anchor_raw,
) = sys.argv[1:]
expected_epoch = int(expected_epoch)

def load(path):
    with open(path, "rb") as source:
        raw = source.read()
    return raw, json.loads(raw)

bundle_raw, bundle = load(bundle_path)
recomputed_raw, recomputed = load(recomputed_path)
receipts_raw, receipts = load(receipts_path)
_, artifact = load(artifact_path)
if bundle.get("epoch") != expected_epoch or recomputed.get("epoch") != expected_epoch:
    raise SystemExit("retained epoch artifacts have the wrong epoch")
if (
    not isinstance(receipts, dict)
    or bundle.get("receipt_snapshot") != receipts
    or bundle.get("receipts") != receipts.get("heads")
    or receipts.get("settlement_epoch") != expected_epoch
    or "epoch" in receipts
):
    raise SystemExit("retained bundle receipts do not match the exact canonical snapshot")
if artifact.get("schema_version") != 1 or artifact.get("type") != "canonical_epoch_artifact":
    raise SystemExit("retained epoch artifact has the wrong schema")
if artifact.get("rail") != "all" or artifact.get("rails") != ["fiat", "tap", "tnk"]:
    raise SystemExit("retained epoch artifact has invalid rail binding")
if artifact.get("epoch") != expected_epoch or artifact.get("epoch_apply_hash") != expected_hash:
    raise SystemExit("retained epoch artifact does not match the canonical epoch/apply hash")
if artifact.get("bundle_sha256") != hashlib.sha256(bundle_raw).hexdigest():
    raise SystemExit("retained epoch bundle hash does not match its artifact")
if artifact.get("recomputed_sha256") != hashlib.sha256(recomputed_raw).hexdigest():
    raise SystemExit("retained recomputed report hash does not match its artifact")
if artifact.get("canonical_receipts_sha256") != hashlib.sha256(receipts_raw).hexdigest():
    raise SystemExit("retained canonical receipt hash does not match its artifact")
roots = recomputed.get("roots")
if not isinstance(roots, dict) or any(
    not re.fullmatch(r"[0-9a-f]{64}", str(roots.get(key, "")))
    for key in ("dep", "use", "earn", "fee", "price")
):
    raise SystemExit("retained recomputed roots are invalid")
if artifact.get("roots") != roots or artifact.get("totals") != recomputed.get("totals"):
    raise SystemExit("retained epoch root evidence does not match recomputation")
totals = recomputed.get("totals")
if not isinstance(totals, dict):
    raise SystemExit("retained epoch totals are invalid")

def confirmed_value(raw, expected_key, label):
    record = json.loads(raw)
    if (
        not isinstance(record, dict)
        or record.get("confirmed") is not True
        or record.get("key") not in (None, expected_key)
        or not isinstance(record.get("value"), dict)
    ):
        raise SystemExit(f"{label} is not confirmed canonical state")
    return record["value"]

apply_state = confirmed_value(
    apply_state_raw,
    "epoch/apply/state",
    "canonical epoch apply state",
)
commit = confirmed_value(
    commit_raw,
    f"epoch/commit/{expected_epoch}",
    "canonical epoch commit",
)
anchor = confirmed_value(
    anchor_raw,
    f"epoch/apply-anchor/{expected_epoch}",
    "canonical epoch apply anchor",
)
commit_hash = commit.get("commit_hash")
if (
    commit.get("type") != "epoch_commit"
    or commit.get("epoch") != expected_epoch
    or commit.get("status") != "provisional"
    or not re.fullmatch(r"[0-9a-f]{64}", str(commit_hash))
    or commit.get("roots") != roots
    or commit.get("totals") != totals
):
    raise SystemExit("canonical epoch commit does not match retained epoch evidence")
if (
    anchor.get("type") != "epoch_apply_anchor"
    or anchor.get("epoch") != expected_epoch
    or anchor.get("apply_hash") != expected_hash
    or not isinstance(anchor.get("settlement_unix"), int)
    or isinstance(anchor.get("settlement_unix"), bool)
    or anchor.get("settlement_unix") < 1
    or not isinstance(anchor.get("applied_at"), str)
    or not anchor.get("applied_at")
):
    raise SystemExit("canonical epoch apply anchor does not match retained epoch evidence")
metadata = receipts.get("metadata")
if (
    not isinstance(metadata, dict)
    or metadata.get("type") != "canonical_receipt_epoch_index"
    or metadata.get("epoch") != expected_epoch
    or metadata.get("count") != len(receipts.get("heads", []))
    or commit["totals"].get("use_count") != metadata.get("count")
    or apply_state.get("updated_epoch") != expected_epoch
    or apply_state.get("pending_epoch") is not None
    or str(apply_state.get("last_apply_hash", "")).lower() != expected_hash
    or apply_state.get("last_receipt_commit_hash") != commit_hash
    or apply_state.get("last_receipt_index_count") != metadata.get("count")
    or apply_state.get("last_receipt_index_revision") != metadata.get("revision")
    or apply_state.get("last_receipt_index_page_count") != metadata.get("page_count")
    or apply_state.get("last_receipt_index_updated_at") != metadata.get("updated_at")
    or apply_state.get("last_receipt_allocation_count") != metadata.get("count")
    or apply_state.get("last_settlement_unix") != anchor.get("settlement_unix")
    or apply_state.get("updated_at") != anchor.get("applied_at")
    or commit.get("at") != anchor.get("settlement_unix")
):
    raise SystemExit("canonical targeted epoch apply does not match retained receipt evidence")
PY
}

validate_epoch_artifact

at_file="$work_dir/settlement-at"
if [[ ! -f "$at_file" ]]; then
  date +%s >"$at_file.tmp"
  mv "$at_file.tmp" "$at_file"
fi
settlement_at="$(cat "$at_file")"
positive_integer "$settlement_at" || {
  echo "abort: invalid frozen settlement timestamp in $at_file" >&2
  exit 1
}
if (( settlement_at < canonical_settlement_unix )); then
  echo "abort: frozen settlement timestamp predates canonical epoch settlement time" >&2
  exit 1
fi

next_attempt() {
  local rail="$1"
  local count_file="$work_dir/$rail.attempts"
  local retry_file="$work_dir/$rail.retry-after"
  local count=0
  if [[ -f "$count_file" ]]; then
    count="$(cat "$count_file")"
  fi
  non_negative_integer "$count" || {
    echo "abort: invalid $rail attempt counter" >&2
    return 1
  }
  if (( count >= MAX_ATTEMPTS )); then
    local now retry_after
    now="$(date +%s)"
    positive_integer "$now" || {
      echo "abort: system clock is invalid while scheduling $rail payout retry" >&2
      return 1
    }
    if [[ ! -f "$retry_file" ]]; then
      retry_after=$((now + RETRY_BACKOFF_SECONDS))
      printf '%s\n' "$retry_after" >"$retry_file.tmp"
      mv "$retry_file.tmp" "$retry_file"
      echo "alert: $rail payout reached $MAX_ATTEMPTS attempts; retry window reopens at $retry_after" >&2
      return 2
    fi
    retry_after="$(cat "$retry_file")"
    positive_integer "$retry_after" || {
      echo "abort: invalid $rail retry-after timestamp" >&2
      return 1
    }
    if (( now < retry_after )); then
      echo "backoff: $rail payout retry window reopens at $retry_after" >&2
      return 2
    fi
    count=0
    rm -f "$retry_file"
    printf '0\n' >"$count_file.tmp"
    mv "$count_file.tmp" "$count_file"
    echo "recovery: reopening $rail payout attempts after bounded backoff" >&2
  fi
  count=$((count + 1))
  printf '%s\n' "$count" >"$count_file.tmp"
  mv "$count_file.tmp" "$count_file"
  printf '%s' "$count"
}

mark_rail() {
  local rail="$1" status="$2" evidence="$3"
  python3 - "$work_dir/$rail.complete.tmp" "$rail" "$status" "$evidence" \
    "$applied_epoch" "$apply_hash" "$canonical_settlement_unix" <<'PY'
import hashlib, json, os, sys
path, rail, status, evidence, epoch, apply_hash, settlement_unix = sys.argv[1:]
if status == "disabled":
    evidence_sha256 = None
elif not os.path.isfile(evidence):
    raise SystemExit(f"{rail} completion evidence is not a file: {evidence}")
else:
    evidence_sha256 = hashlib.sha256(open(evidence, "rb").read()).hexdigest()
with open(path, "w") as out:
    json.dump({
        "schema_version": 1,
        "rail": rail,
        "status": status,
        "epoch": int(epoch),
        "epoch_apply_hash": apply_hash,
        "canonical_settlement_unix": int(settlement_unix),
        "evidence": evidence,
        "evidence_sha256": evidence_sha256,
    }, out, indent=2)
    out.write("\n")
PY
  mv "$work_dir/$rail.complete.tmp" "$work_dir/$rail.complete"
}

validate_rail_marker() {
  local rail="$1"
  python3 - "$work_dir/$rail.complete" "$rail" "$applied_epoch" "$apply_hash" \
    "$canonical_settlement_unix" <<'PY'
import hashlib, json, os, sys
path, expected_rail, expected_epoch, expected_hash, expected_settlement_unix = sys.argv[1:]
d = json.load(open(path))
if (
    d.get("schema_version") != 1
    or d.get("rail") != expected_rail
    or d.get("epoch") != int(expected_epoch)
    or d.get("epoch_apply_hash") != expected_hash
    or d.get("canonical_settlement_unix") != int(expected_settlement_unix)
    or d.get("status") not in {"settled", "already_settled", "carry", "no_work", "disabled"}
):
    raise SystemExit(f"{expected_rail} completion marker is stale or malformed")
if d["status"] == "disabled":
    if d.get("evidence_sha256") is not None:
        raise SystemExit(f"{expected_rail} disabled marker has file evidence")
else:
    evidence = d.get("evidence")
    if not isinstance(evidence, str) or not os.path.isfile(evidence):
        raise SystemExit(f"{expected_rail} completion evidence is missing")
    if hashlib.sha256(open(evidence, "rb").read()).hexdigest() != d.get("evidence_sha256"):
        raise SystemExit(f"{expected_rail} completion evidence hash changed")
PY
}

mark_tap_produced() {
  local evidence="$1"
  python3 - "$work_dir/tap.produced.tmp" "$evidence" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, os, sys
path, evidence, epoch, apply_hash = sys.argv[1:]
if not os.path.isfile(evidence):
    raise SystemExit("TAP produced evidence is not a spool file")
with open(path, "w") as out:
    json.dump({
        "schema_version": 1,
        "rail": "tap",
        "status": "produced",
        "epoch": int(epoch),
        "epoch_apply_hash": apply_hash,
        "evidence": evidence,
        "evidence_sha256": hashlib.sha256(open(evidence, "rb").read()).hexdigest(),
    }, out, indent=2)
    out.write("\n")
PY
  mv "$work_dir/tap.produced.tmp" "$work_dir/tap.produced"
}

validate_settlement_report() {
  local report="$1" rail="$2" phase="$3"
  python3 - "$report" "$rail" "$phase" "$applied_epoch" "$apply_hash" "$SOURCE_DIR" <<'PY'
import json, re, subprocess, sys
path, rail, phase, expected_epoch, expected_hash, source_dir = sys.argv[1:]
expected_epoch = int(expected_epoch)
d = json.load(open(path))
if d.get("ok") is not True or d.get("epoch") != expected_epoch:
    raise SystemExit(f"{rail} report outer epoch or ok flag is invalid")
settlement = d.get("settlement")
already = d.get("already_settled")
if not isinstance(settlement, dict):
    raise SystemExit(f"{rail} report is missing settlement payload")
if (
    already is not None
    and settlement.get("op") != "prepare_targeted_payout_epoch"
    and already != settlement
):
    raise SystemExit(f"{rail} report outer and retained settlement evidence disagree")
expected_op = "prepare_targeted_payout_epoch"
if (
    settlement.get("op") != expected_op
    or settlement.get("rail") != rail
    or settlement.get("epoch") != expected_epoch
    or str(settlement.get("epoch_apply_hash", "")).lower() != expected_hash
):
    raise SystemExit(f"{rail} settlement payload is stale or rail-contaminated")

skipped = d.get("skipped_providers")
if not isinstance(skipped, list):
    raise SystemExit(f"{rail} report is missing explicit skipped-provider evidence")
for item in skipped:
    if (
        not isinstance(item, dict)
        or not {"provider", "payout_revision", "au", "reason", "blocking"}.issubset(item)
        or not set(item).issubset({
            "provider", "payout_revision", "au", "payout_min_au", "reason", "blocking"
        })
        or not re.fullmatch(r"[0-9a-f]{64}", str(item.get("provider", "")))
        or not re.fullmatch(r"[0-9a-f]{64}", str(item.get("payout_revision", "")))
        or not re.fullmatch(r"(0|[1-9][0-9]*)", str(item.get("au", "")))
        or not isinstance(item.get("reason"), str)
        or not isinstance(item.get("blocking"), bool)
    ):
        raise SystemExit(f"{rail} skipped-provider evidence is malformed")
    if item["blocking"] is False:
        minimum = item.get("payout_min_au")
        if (
            item["reason"] !=
                "liability is below canonical payout_min_au and remains carried forward"
            or not isinstance(minimum, str)
            or not re.fullmatch(r"(0|[1-9][0-9]*)", minimum)
            or not (0 < int(item["au"]) < int(minimum))
        ):
            raise SystemExit(f"{rail} nonblocking carry evidence omits payable work")

no_work = d.get("no_work")
carry_forward = d.get("carry_forward")
if not isinstance(no_work, bool) or not isinstance(carry_forward, bool):
    raise SystemExit(f"{rail} report lacks explicit no_work/carry_forward booleans")

if settlement.get("op") == "prepare_targeted_payout_epoch":
    hash_pattern = re.compile(r"^[0-9a-f]{64}$")
    plan_fields = {
        "op", "contract_version", "rail", "epoch", "at",
        "epoch_apply_hash", "snapshot_signed_length", "outcome", "outputs",
        "carry", "outputs_root", "carry_root", "plan_root", "admin", "admin_sig",
    }
    if set(settlement) != plan_fields:
        raise SystemExit(f"{rail} immutable epoch plan does not have the exact v17 schema")
    if (
        settlement.get("rail") != rail
        or settlement.get("epoch") != expected_epoch
        or settlement.get("epoch_apply_hash") != expected_hash
        or not isinstance(settlement.get("contract_version"), int)
        or isinstance(settlement.get("contract_version"), bool)
        or settlement.get("contract_version") < 17
        or not isinstance(settlement.get("at"), int)
        or isinstance(settlement.get("at"), bool)
        or settlement.get("at") < 1
        or not isinstance(settlement.get("snapshot_signed_length"), int)
        or isinstance(settlement.get("snapshot_signed_length"), bool)
        or settlement.get("snapshot_signed_length") < 1
        or any(
            not isinstance(settlement.get(field), str)
            or not hash_pattern.fullmatch(settlement[field])
            for field in ("outputs_root", "carry_root", "plan_root")
        )
    ):
        raise SystemExit(f"{rail} immutable epoch plan identity is invalid")

    outputs = settlement.get("outputs")
    carry = settlement.get("carry")
    if not isinstance(outputs, list) or not isinstance(carry, list):
        raise SystemExit(f"{rail} immutable epoch plan lacks outputs or carry")
    economic_ids = []
    for index, output in enumerate(outputs):
        if (
            not isinstance(output, dict)
            or output.get("output_index") != index
            or not isinstance(output.get("economic_op_id"), str)
            or not hash_pattern.fullmatch(output["economic_op_id"])
        ):
            raise SystemExit(f"{rail} immutable epoch output identity is invalid")
        economic_ids.append(output["economic_op_id"])
    if len(set(economic_ids)) != len(economic_ids):
        raise SystemExit(f"{rail} immutable epoch output identities are duplicated")
    if any(not isinstance(item, dict) for item in carry):
        raise SystemExit(f"{rail} immutable epoch carry evidence is malformed")
    expected_outcome = "payouts" if outputs else ("carry" if carry else "no_work")
    if settlement.get("outcome") != expected_outcome:
        raise SystemExit(f"{rail} immutable epoch plan outcome is inconsistent")
    expected_no_work = already is None and (
        expected_outcome == "no_work"
        if rail == "fiat"
        else not outputs
    )
    expected_carry = already is None and (
        expected_outcome == "carry"
        if rail == "fiat"
        else not outputs
    )
    if no_work != expected_no_work or carry_forward != expected_carry:
        raise SystemExit(f"{rail} report no_work/carry flags disagree with its immutable plan")

    def record_value(record, expected_type, label):
        if (
            not isinstance(record, dict)
            or record.get("type") != expected_type
            or not isinstance(record.get("value"), dict)
        ):
            raise SystemExit(f"{rail} {label} readback is malformed")
        return record["value"]

    def validate_close(record):
        value = record_value(record, "targeted_payout_epoch_close", "canonical close")
        if (
            record.get("rail") != rail
            or record.get("epoch") != expected_epoch
            or record.get("plan_root") != settlement["plan_root"]
            or record.get("outcome") != expected_outcome
            or record.get("output_count") != len(outputs)
            or record.get("carry_count") != len(carry)
            or record.get("outputs_root") != settlement["outputs_root"]
            or record.get("carry_root") != settlement["carry_root"]
            or value.get("op") != "close_targeted_payout_epoch"
            or value.get("rail") != rail
            or value.get("epoch") != expected_epoch
            or value.get("epoch_apply_hash") != expected_hash
            or value.get("plan_root") != settlement["plan_root"]
        ):
            raise SystemExit(f"{rail} canonical close does not match its immutable plan")

    if already is not None:
        validate_close(already)
        if d.get("submitted") is True:
            raise SystemExit(f"{rail} replay unexpectedly claims a new submit")
        raise SystemExit(0)

    if phase == "plan":
        if d.get("submitted") is not False:
            raise SystemExit(f"{rail} plan unexpectedly claims a submit")
        raise SystemExit(0)

    result = d.get("feature_result")
    if not isinstance(result, dict):
        raise SystemExit(f"{rail} final report is missing canonical result readbacks")
    required_result_fields = {"plan", "outputs", "close"}
    if not required_result_fields.issubset(result):
        raise SystemExit(f"{rail} final report is missing plan/output/close evidence")

    plan_value = record_value(
        result.get("plan"),
        "targeted_payout_epoch_plan",
        "canonical plan",
    )
    if plan_value != settlement:
        raise SystemExit(f"{rail} canonical plan readback differs from the immutable plan")

    output_records = result.get("outputs")
    if not isinstance(output_records, list) or len(output_records) != len(outputs):
        raise SystemExit(f"{rail} final report does not cover every planned output")
    output_type = (
        "targeted_fiat_output_settlement"
        if rail == "fiat"
        else "targeted_tnk_output_settlement"
    )
    for output, record in zip(outputs, output_records):
        value = record_value(record, output_type, "output settlement")
        if (
            record.get("rail") != rail
            or record.get("epoch") != expected_epoch
            or record.get("economic_op_id") != output["economic_op_id"]
            or value.get("rail") != rail
            or value.get("epoch") != expected_epoch
            or value.get("epoch_apply_hash") != expected_hash
            or value.get("plan_root") != settlement["plan_root"]
            or value.get("economic_op_id") != output["economic_op_id"]
            or value.get("output_index") != output["output_index"]
            or (
                rail == "fiat"
                and (
                    not isinstance(value.get("attempt_id"), str)
                    or not hash_pattern.fullmatch(value["attempt_id"])
                )
            )
        ):
            raise SystemExit(f"{rail} output settlement does not match its planned output")

    validate_close(result.get("close"))
    if d.get("settlement_state") != result.get("close"):
        raise SystemExit(f"{rail} final settlement state differs from its canonical close")
    if d.get("submitted") is not True:
        raise SystemExit(f"{rail} final report does not prove canonical submission")
    raise SystemExit(0)

raise SystemExit(f"{rail} report does not use the v17 immutable epoch-plan schema")

if rail == "fiat":
    payload_keys = {
        "op", "epoch", "at", "rail", "processor", "source_currency",
        "operator_to", "epoch_apply_hash", "stripe_transfers", "transfer_root",
        "provider_count", "provider_liability_au", "provider_paid_au",
        "operator_fee_liability_au", "operator_fee_retained_au",
        "gross_liability_au", "gross_paid_au", "rounding_au", "dust_au",
        "source_amount_minor", "destination_totals", "outputs",
    }
    prepared_payload_keys = payload_keys | {"preparation_ids", "external_effect_ids"}
    state_keys = prepared_payload_keys | {"type", "settled_by", "settled_by_role"}
    decimal_pattern = re.compile(r"^(0|[1-9][0-9]*)$")
    rate_pattern = re.compile(r"^(0|[1-9][0-9]*)(\.[0-9]+)?$")
    currency_pattern = re.compile(r"^[a-z]{3}$")
    hash_pattern = re.compile(r"^[0-9a-f]{64}$")

    def exact_keys(value, expected, label):
        if not isinstance(value, dict) or set(value) != expected:
            raise SystemExit(f"{label} does not have the exact canonical schema")

    def exact_keys_with_null_optional(value, required, optional, label):
        if (
            not isinstance(value, dict)
            or not required.issubset(value)
            or not set(value).issubset(required | optional)
            or any(value.get(key) is not None for key in optional)
        ):
            raise SystemExit(f"{label} does not have the exact canonical schema")

    def decimal(value, label, *, positive=False):
        if not isinstance(value, str) or not decimal_pattern.fullmatch(value):
            raise SystemExit(f"{label} is not a canonical decimal string")
        parsed = int(value)
        if positive and parsed == 0:
            raise SystemExit(f"{label} must be positive")
        return parsed

    def json_minor(value, label, *, positive=False):
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise SystemExit(f"{label} is not a canonical JSON minor-unit integer")
        if positive and value == 0:
            raise SystemExit(f"{label} must be positive")
        return value

    def exact_rate_ratio(value, label):
        if (
            not isinstance(value, str)
            or not rate_pattern.fullmatch(value)
            or not any(character in "123456789" for character in value)
        ):
            raise SystemExit(f"{label} is not a positive exact decimal rate")
        whole, separator, fraction = value.partition(".")
        denominator = 10 ** len(fraction) if separator else 1
        numerator = int(whole) * denominator + (int(fraction) if separator else 0)
        return numerator, denominator

    def exact_rates_equal(left, right):
        left_numerator, left_denominator = left
        right_numerator, right_denominator = right
        return left_numerator * right_denominator == right_numerator * left_denominator

    def currency(value, label):
        if not isinstance(value, str) or not currency_pattern.fullmatch(value):
            raise SystemExit(f"{label} is not a canonical currency")
        return value

    def hash32(value, label):
        if not isinstance(value, str) or not hash_pattern.fullmatch(value):
            raise SystemExit(f"{label} is not a canonical 32-byte hash")
        return value

    def stripe_quote_hashes(reports):
        quotes = [
            report.get("quote")
            for report in reports
            if isinstance(report, dict) and isinstance(report.get("quote"), dict)
        ]
        if not quotes:
            return {}
        canonical = [
            json.dumps(
                {
                    "domain": "mayhem-stripe-fx-quote-evidence-v1",
                    "value": quote,
                },
                ensure_ascii=False,
                separators=(",", ":"),
                sort_keys=True,
            )
            for quote in quotes
        ]
        helper = r'''
const path = require("node:path");
const { createRequire } = require("node:module");
const sourceDir = process.argv[1];
const requireFromRoot = createRequire(path.join(sourceDir, "scripts/ops-payout-settle.sh"));
const { blake3 } = requireFromRoot(
  path.join(sourceDir, "intercom/node_modules/@tracsystems/blake3")
);
let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => { input += chunk; });
process.stdin.on("end", async () => {
  const values = JSON.parse(input);
  const hashes = [];
  for (const value of values) {
    hashes.push(Buffer.from(await blake3(Buffer.from(value, "utf8"))).toString("hex"));
  }
  process.stdout.write(JSON.stringify(hashes));
});
'''
        try:
            completed = subprocess.run(
                ["node", "-e", helper, source_dir],
                input=json.dumps(canonical, separators=(",", ":")),
                capture_output=True,
                check=False,
                text=True,
            )
            hashes = json.loads(completed.stdout) if completed.returncode == 0 else None
        except (OSError, json.JSONDecodeError):
            hashes = None
        if (
            not isinstance(hashes, list)
            or len(hashes) != len(quotes)
            or any(not isinstance(value, str) or not hash_pattern.fullmatch(value) for value in hashes)
        ):
            raise SystemExit("fiat Stripe quote readback hashing failed")
        return {
            id(quote): digest
            for quote, digest in zip(quotes, hashes)
        }

    def validate_quote_readback(quote, output, expected_hash, computed_hash):
        exact_keys(
            quote,
            {
                "id", "created", "expires_at", "lock_duration", "lock_status",
                "to_currency", "usage", "rates",
            },
            "fiat Stripe FX quote readback",
        )
        usage = quote.get("usage")
        rates = quote.get("rates")
        if (
            quote.get("id") != output.get("fx_quote_id")
            or not isinstance(quote.get("created"), int)
            or isinstance(quote.get("created"), bool)
            or quote["created"] < 0
            or not isinstance(quote.get("expires_at"), int)
            or isinstance(quote.get("expires_at"), bool)
            or quote["expires_at"] <= quote["created"]
            or quote.get("lock_duration") != "five_minutes"
            or not isinstance(quote.get("lock_status"), str)
            or not quote["lock_status"]
            or quote.get("to_currency") != output["destination_currency"]
            or not isinstance(usage, dict)
            or set(usage) != {"type", "destination"}
            or usage.get("type") != "transfer"
            or usage.get("destination") != output["to"]
            or not isinstance(rates, dict)
            or not rates
        ):
            raise SystemExit("fiat FX quote readback disagrees with output")
        required_rates = {"usd"}
        if output["source_currency"] != output["destination_currency"]:
            required_rates.add(output["source_currency"])
        if not required_rates.issubset(rates):
            raise SystemExit("fiat FX quote readback lacks required valuation rates")
        for key, rate in rates.items():
            currency(key, "fiat FX quote rate currency")
            exact_keys(
                rate,
                {"exchange_rate", "base_rate"},
                "fiat Stripe FX quote rate",
            )
            for field in ("exchange_rate", "base_rate"):
                value = rate.get(field)
                if (
                    not isinstance(value, str)
                    or not rate_pattern.fullmatch(value)
                    or not any(character in "123456789" for character in value)
                ):
                    raise SystemExit("fiat Stripe FX quote rate is invalid")
        if computed_hash != expected_hash:
            raise SystemExit("fiat FX quote readback hash disagrees with retained evidence")

    def validate_base(value, *, retained=False, prepared=False):
        expected_keys = state_keys if retained else (
            prepared_payload_keys if prepared else payload_keys
        )
        exact_keys(value, expected_keys, "fiat settlement")
        if (
            value.get("op") != "settle_targeted_fiat"
            or value.get("rail") != "fiat"
            or value.get("processor") != "stripe"
            or value.get("epoch") != expected_epoch
            or not isinstance(value.get("at"), int)
            or isinstance(value.get("at"), bool)
            or value["at"] < 0
            or not isinstance(value.get("operator_to"), str)
            or not value["operator_to"]
            or value.get("epoch_apply_hash") != expected_hash
        ):
            raise SystemExit("fiat settlement identity is invalid")
        if retained:
            if (
                value.get("type") != "targeted_fiat_settlement"
                or value.get("settled_by_role") != "admin"
            ):
                raise SystemExit("fiat retained state metadata is invalid")
            hash32(value.get("settled_by"), "fiat retained state writer")

    def validate_draft(value):
        validate_base(value)
        if (
            value.get("source_currency") is not None
            or value.get("transfer_root") is not None
            or value.get("source_amount_minor") != "0"
            or value.get("destination_totals") != []
            or value.get("outputs") != []
            or value.get("stripe_transfers") != []
        ):
            raise SystemExit("fiat plan is neither an exact draft nor exact settlement")
        count = value.get("provider_count")
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            raise SystemExit("fiat draft provider count is invalid")
        for field in (
            "provider_liability_au", "provider_paid_au",
            "operator_fee_liability_au", "operator_fee_retained_au",
            "gross_liability_au", "gross_paid_au", "rounding_au", "dust_au",
        ):
            decimal(value.get(field), f"fiat draft {field}")

    def validate_exact(value, *, retained=False):
        candidate_outputs = value.get("outputs")
        preparation_ids = value.get("preparation_ids")
        effect_ids = value.get("external_effect_ids")
        output_count = len(candidate_outputs) if isinstance(candidate_outputs, list) else -1
        if (
            not isinstance(preparation_ids, list)
            or len(preparation_ids) != output_count
            or len(set(preparation_ids)) != output_count
            or any(not isinstance(item, str) or not hash_pattern.fullmatch(item)
                   for item in preparation_ids)
        ):
            raise SystemExit(
                "fiat final settlement lacks one unique 32-byte preparation id per output"
            )
        if (
            not isinstance(effect_ids, list)
            or len(effect_ids) != output_count
            or len(set(effect_ids)) != output_count
            or any(not isinstance(item, str) or not hash_pattern.fullmatch(item)
                   for item in effect_ids)
        ):
            raise SystemExit(
                "fiat final settlement lacks one unique 32-byte external effect id per output"
            )
        validate_base(value, retained=retained, prepared=True)
        source_currency = currency(value.get("source_currency"), "fiat source currency")
        source_amount_minor = decimal(
            value.get("source_amount_minor"),
            "fiat source amount",
            positive=True,
        )
        hash32(value.get("transfer_root"), "fiat transfer root")
        count = value.get("provider_count")
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            raise SystemExit("fiat provider count is invalid")
        top = {
            field: decimal(
                value.get(field),
                f"fiat {field}",
                positive=field in {"gross_liability_au", "gross_paid_au"},
            )
            for field in (
                "provider_liability_au", "provider_paid_au",
                "operator_fee_liability_au", "operator_fee_retained_au",
                "gross_liability_au", "gross_paid_au", "rounding_au", "dust_au",
            )
        }
        if (
            top["provider_liability_au"] + top["operator_fee_liability_au"]
                != top["gross_liability_au"]
            or top["provider_paid_au"] + top["operator_fee_retained_au"]
                != top["gross_paid_au"]
            or top["gross_paid_au"] + top["dust_au"] != top["gross_liability_au"]
            or top["rounding_au"] != top["dust_au"]
        ):
            raise SystemExit("fiat canonical AU totals do not balance")

        outputs = value.get("outputs")
        transfers = value.get("stripe_transfers")
        if (
            not isinstance(outputs, list)
            or not outputs
            or not isinstance(transfers, list)
            or len(transfers) != len(outputs)
        ):
            raise SystemExit("fiat outputs or retained Stripe evidence is incomplete")

        provider_count = 0
        provider_liability = 0
        provider_paid = 0
        operator_liability = 0
        operator_paid = 0
        rounding = 0
        dust = 0
        source_minor = 0
        destination_totals = {}
        provider_order = []
        operator_seen = False
        refs = set()
        quotes = set()
        destination_payments = set()
        expected_group = f"mayhem_fiat_epoch_{expected_epoch}_{expected_hash[:16]}"

        for index, (output, evidence) in enumerate(zip(outputs, transfers)):
            role = output.get("role") if isinstance(output, dict) else None
            if role == "provider":
                output_fields = {
                    "role", "provider", "payout_revision", "to", "liability_au",
                    "paid_au", "rounding_au", "dust_au", "source_currency",
                    "source_amount_minor", "destination_currency",
                    "destination_amount_minor",
                }
                quote_fields = {"fx_quote_id", "fx_quote_hash"}
                direct_usd = (
                    output.get("source_currency") == "usd"
                    and output.get("destination_currency") == "usd"
                )
                if direct_usd:
                    exact_keys_with_null_optional(
                        output,
                        output_fields,
                        quote_fields,
                        "fiat provider output",
                    )
                else:
                    exact_keys(output, output_fields | quote_fields, "fiat provider output")
                provider = hash32(output.get("provider"), "fiat provider identity")
                revision = hash32(output.get("payout_revision"), "fiat payout revision")
                provider_order.append((provider, revision))
                if operator_seen:
                    raise SystemExit("fiat provider output follows operator output")
                provider_count += 1
            elif role == "operator_fee":
                output_fields = {
                    "role", "to", "liability_au", "paid_au", "rounding_au",
                    "dust_au", "source_currency", "source_amount_minor",
                }
                exact_keys(output, output_fields, "fiat operator output")
                if operator_seen:
                    raise SystemExit("fiat settlement has duplicate operator output")
                operator_seen = True
                if output.get("to") != value.get("operator_to"):
                    raise SystemExit("fiat operator output target is inconsistent")
            else:
                raise SystemExit("fiat settlement output role is invalid")

            if not isinstance(output.get("to"), str) or not output["to"]:
                raise SystemExit("fiat settlement output target is invalid")
            if currency(output.get("source_currency"), "fiat output source currency") != source_currency:
                raise SystemExit("fiat output source currency disagrees with settlement")
            output_source_minor = decimal(
                output.get("source_amount_minor"),
                "fiat output source amount",
                positive=True,
            )
            liability = decimal(output.get("liability_au"), "fiat output liability", positive=True)
            paid = decimal(output.get("paid_au"), "fiat output paid amount", positive=True)
            output_rounding = decimal(output.get("rounding_au"), "fiat output rounding")
            output_dust = decimal(output.get("dust_au"), "fiat output dust")
            if paid + output_dust != liability or output_rounding != output_dust:
                raise SystemExit("fiat output AU liability, paid amount, rounding, and dust do not balance")
            source_minor += output_source_minor
            rounding += output_rounding
            dust += output_dust

            if role == "provider":
                provider_liability += liability
                provider_paid += paid
                destination_currency = currency(
                    output.get("destination_currency"),
                    "fiat provider destination currency",
                )
                destination_minor = decimal(
                    output.get("destination_amount_minor"),
                    "fiat provider destination amount",
                    positive=True,
                )
                destination_totals[destination_currency] = (
                    destination_totals.get(destination_currency, 0) + destination_minor
                )
                same_currency = destination_currency == source_currency
                if same_currency:
                    if destination_minor != output_source_minor:
                        raise SystemExit(
                            "fiat same-currency provider source and destination amounts differ"
                        )
                direct_usd = same_currency and source_currency == "usd"
                if direct_usd:
                    if (
                        output.get("fx_quote_id") is not None
                        or output.get("fx_quote_hash") is not None
                    ):
                        raise SystemExit("fiat direct-USD provider output retains an FX quote")
                    quote_id = None
                    quote_hash = None
                else:
                    quote_id = output.get("fx_quote_id")
                    if (
                        not isinstance(quote_id, str)
                        or not re.fullmatch(r"fxq_[A-Za-z0-9._-]+", quote_id)
                    ):
                        raise SystemExit("fiat provider FX quote id is invalid")
                    quote_hash = hash32(
                        output.get("fx_quote_hash"),
                        "fiat provider FX quote hash",
                    )
                evidence_fields = {
                    "schema_version", "kind", "ref", "destination", "source_currency",
                    "source_amount_minor", "destination_currency",
                    "destination_amount_minor", "destination_payment", "transfer_group",
                }
                if direct_usd:
                    exact_keys_with_null_optional(
                        evidence,
                        evidence_fields,
                        quote_fields,
                        "fiat retained Stripe transfer evidence",
                    )
                else:
                    exact_keys(
                        evidence,
                        evidence_fields | quote_fields,
                        "fiat retained Stripe transfer evidence",
                    )
            else:
                operator_liability += liability
                operator_paid += paid
                evidence_fields = {
                    "schema_version", "kind", "ref", "destination", "source_currency",
                    "source_amount_minor", "transfer_group",
                }

                exact_keys(evidence, evidence_fields, "fiat retained Stripe transfer evidence")
            if (
                evidence.get("schema_version") != 2
                or evidence.get("destination") != output.get("to")
                or evidence.get("source_currency") != source_currency
                or evidence.get("source_amount_minor") != output.get("source_amount_minor")
            ):
                raise SystemExit("fiat retained Stripe evidence disagrees with output")
            ref = evidence.get("ref")
            if not isinstance(ref, str) or ref in refs:
                raise SystemExit("fiat retained Stripe transfer reference is invalid or duplicated")
            refs.add(ref)
            if role == "provider":
                destination_payment = evidence.get("destination_payment")
                if (
                    evidence.get("kind") != "stripe_transfer"
                    or not ref.startswith("tr_")
                    or evidence.get("destination_currency") != output.get("destination_currency")
                    or evidence.get("destination_amount_minor") != output.get("destination_amount_minor")
                    or evidence.get("fx_quote_id") != quote_id
                    or evidence.get("fx_quote_hash") != quote_hash
                    or evidence.get("transfer_group") != expected_group
                    or not isinstance(destination_payment, str)
                    or not re.fullmatch(r"py_[A-Za-z0-9._-]+", destination_payment)
                    or (quote_id is not None and quote_id in quotes)
                    or destination_payment in destination_payments
                ):
                    raise SystemExit("fiat provider FX/transfer evidence is inconsistent")
                if quote_id is not None:
                    quotes.add(quote_id)
                destination_payments.add(destination_payment)
            elif (
                evidence.get("kind") != "platform_balance"
                or not ref.startswith("platform_balance:")
                or evidence.get("transfer_group") is not None
            ):
                raise SystemExit("fiat operator retained-balance evidence is inconsistent")

        if provider_order != sorted(provider_order):
            raise SystemExit("fiat provider outputs are not canonically ordered")
        expected_destinations = [
            {"currency": key, "amount_minor": str(destination_totals[key])}
            for key in sorted(destination_totals)
        ]
        if value.get("destination_totals") != expected_destinations:
            raise SystemExit("fiat destination totals do not match provider outputs")
        if (
            count != provider_count
            or source_amount_minor != source_minor
            or top["provider_liability_au"] != provider_liability
            or top["provider_paid_au"] != provider_paid
            or top["operator_fee_liability_au"] != operator_liability
            or top["operator_fee_retained_au"] != operator_paid
            or top["rounding_au"] != rounding
            or top["dust_au"] != dust
        ):
            raise SystemExit("fiat settlement totals do not match outputs")
        return outputs, transfers

    def validate_preparation_barrier(value, outputs):
        preparation_ids = value.get("preparation_ids")
        effect_ids = value.get("external_effect_ids")
        if (
            not isinstance(preparation_ids, list)
            or len(preparation_ids) != len(outputs)
            or len(set(preparation_ids)) != len(outputs)
            or any(not isinstance(item, str) or not hash_pattern.fullmatch(item)
                   for item in preparation_ids)
        ):
            raise SystemExit(
                "fiat final settlement lacks one unique 32-byte preparation id per output"
            )
        if (
            not isinstance(effect_ids, list)
            or len(effect_ids) != len(outputs)
            or len(set(effect_ids)) != len(outputs)
            or any(not isinstance(item, str) or not hash_pattern.fullmatch(item)
                   for item in effect_ids)
        ):
            raise SystemExit(
                "fiat final settlement lacks one unique 32-byte external effect id per output"
            )
        preparations = d.get("payout_preparations")
        if not isinstance(preparations, list) or len(preparations) != len(outputs):
            raise SystemExit(
                "fiat final report lacks one canonical preparation readback per output"
            )
        by_id = {
            item.get("economic_op_id"): item
            for item in preparations
            if isinstance(item, dict)
        }
        if len(by_id) != len(outputs):
            raise SystemExit("fiat canonical preparation readbacks are duplicated or malformed")
        for index, (preparation_id, effect_id) in enumerate(
            zip(preparation_ids, effect_ids)
        ):
            preparation = by_id.get(preparation_id)
            if (
                not isinstance(preparation, dict)
                or preparation.get("type") != "targeted_payout_preparation"
                or preparation.get("rail") != "fiat"
                or preparation.get("epoch") != expected_epoch
                or preparation.get("epoch_apply_hash") != expected_hash
                or preparation.get("output_index") != index
                or preparation.get("external_effect_ids") != [effect_id]
                or preparation.get("consumed") is not False
            ):
                raise SystemExit(
                    "fiat canonical preparation readback does not bind its output and effect"
                )

    def validate_readback(value, outputs, transfers):
        reports = d.get("stripe_transfers")
        if not isinstance(reports, list) or len(reports) != len(outputs):
            raise SystemExit("fiat Stripe readback reports do not cover every output")
        platform = d.get("platform_account")
        exact_keys(
            platform,
            {"id", "default_currency", "livemode", "attempts"},
            "fiat Stripe platform account readback",
        )
        platform_attempts = platform.get("attempts")
        if (
            not isinstance(platform.get("id"), str)
            or not re.fullmatch(r"acct_[A-Za-z0-9._-]+", platform["id"])
            or currency(
                platform.get("default_currency"),
                "fiat Stripe platform default currency",
            ) != value["source_currency"]
            or not isinstance(platform.get("livemode"), bool)
            or not isinstance(platform_attempts, int)
            or isinstance(platform_attempts, bool)
            or platform_attempts <= 0
        ):
            raise SystemExit("fiat settlement source disagrees with Stripe platform account")
        readback_quote_hashes = stripe_quote_hashes(reports)
        provider_reports = {}
        provider_indexes = set()
        operator_reports = []
        for report in reports:
            if not isinstance(report, dict):
                raise SystemExit("fiat Stripe readback report is invalid")
            index = report.get("output_index")
            if isinstance(index, int) and not isinstance(index, bool):
                transfer = report.get("transfer")
                ref = transfer.get("id") if isinstance(transfer, dict) else None
                if index < 0 or index in provider_indexes or not isinstance(ref, str) or ref in provider_reports:
                    raise SystemExit("fiat Stripe readback has a duplicate output index")
                provider_indexes.add(index)
                provider_reports[ref] = report
            elif index is None and report.get("kind") == "platform_balance":
                operator_reports.append(report)
            else:
                raise SystemExit("fiat Stripe readback output index is invalid")

        operator_count = 0
        provider_source = {}
        provider_destination = {}
        operator_source = {}
        for index, (output, evidence) in enumerate(zip(outputs, transfers)):
            if output["role"] == "operator_fee":
                operator_count += 1
                if len(operator_reports) != operator_count:
                    raise SystemExit("fiat operator retained-balance readback is missing")
                report = operator_reports[operator_count - 1]
                if (
                    report.get("source_currency") != output["source_currency"]
                    or report.get("source_amount_minor") != output["source_amount_minor"]
                    or report.get("liability_au") != output["liability_au"]
                    or report.get("retained_au") != output["paid_au"]
                    or report.get("dust_au") != output["dust_au"]
                ):
                    raise SystemExit("fiat operator readback disagrees with retained output")
                operator_source[output["source_currency"]] = (
                    operator_source.get(output["source_currency"], 0)
                    + int(output["source_amount_minor"])
                )
                continue

            report = provider_reports.get(evidence["ref"])
            if not isinstance(report, dict):
                raise SystemExit("fiat provider Stripe readback is missing")
            account = report.get("account")
            fx = report.get("fx")
            quote = report.get("quote")
            transfer = report.get("transfer")
            payment = report.get("destination_payment")
            if not all(isinstance(item, dict) for item in (account, fx, transfer, payment)):
                raise SystemExit("fiat provider Stripe readback is incomplete")
            same_currency = output["source_currency"] == output["destination_currency"]
            direct_usd = same_currency and output["source_currency"] == "usd"
            if direct_usd:
                if quote is not None:
                    raise SystemExit("fiat direct-USD provider has an FX quote readback")
            elif not isinstance(quote, dict):
                raise SystemExit("fiat provider Stripe readback is incomplete")
            if (
                account.get("id") != output["to"]
                or account.get("default_currency") != output["destination_currency"]
                or account.get("ready") is not True
                or fx.get("liability_au") != output["liability_au"]
                or fx.get("paid_au") != output["paid_au"]
                or fx.get("rounding_au") != output["rounding_au"]
                or fx.get("dust_au") != output["dust_au"]
                or fx.get("source_currency") != output["source_currency"]
                or fx.get("source_amount_minor") != output["source_amount_minor"]
                or fx.get("destination_currency") != output["destination_currency"]
            ):
                raise SystemExit("fiat provider account/FX readback disagrees with output")
            if same_currency:
                for field in (
                    "target_destination_amount_minor",
                    "maximum_destination_amount_minor",
                ):
                    amount = fx.get(field)
                    if amount is not None and amount != output["destination_amount_minor"]:
                        raise SystemExit(
                            "fiat same-currency account/transfer readback is not exact"
                        )
            else:
                target_minor = decimal(
                    fx.get("target_destination_amount_minor"),
                    "fiat FX target destination amount",
                    positive=True,
                )
                maximum_minor = decimal(
                    fx.get("maximum_destination_amount_minor"),
                    "fiat FX maximum destination amount",
                    positive=True,
                )
                actual_minor = int(output["destination_amount_minor"])
                if target_minor > actual_minor or actual_minor > maximum_minor:
                    raise SystemExit("fiat destination payment falls outside its exact FX bound")
            if not direct_usd:
                validate_quote_readback(
                    quote,
                    output,
                    output["fx_quote_hash"],
                    readback_quote_hashes.get(id(quote)),
                )
            expected_transfer_quote = None if same_currency else output["fx_quote_id"]
            if (
                transfer.get("id") != evidence["ref"]
                or transfer.get("source_currency") != output["source_currency"]
                or str(transfer.get("source_amount_minor")) != output["source_amount_minor"]
                or transfer.get("destination") != output["to"]
                or transfer.get("destination_payment") != evidence["destination_payment"]
                or (same_currency and not direct_usd and "fx_quote" not in transfer)
                or transfer.get("fx_quote") != expected_transfer_quote
                or transfer.get("transfer_group") != evidence["transfer_group"]
                or transfer.get("verified") is not True
                or transfer.get("reversed") is not False
                or transfer.get("amount_reversed") != 0
            ):
                raise SystemExit("fiat Stripe transfer readback disagrees with retained evidence")
            exact_keys(
                payment,
                {
                    "id", "source_amount_minor", "source_currency", "amount_minor",
                    "gross_amount_minor", "currency", "fee_minor", "net_minor",
                    "exchange_rate", "paid", "captured", "source_transfer",
                    "balance_transaction",
                },
                "fiat Stripe destination-payment readback",
            )
            payment_source_minor = json_minor(
                payment.get("source_amount_minor"),
                "fiat destination-payment source amount",
                positive=True,
            )
            payment_amount_minor = json_minor(
                payment.get("amount_minor"),
                "fiat destination-payment net amount",
                positive=True,
            )
            payment_gross_minor = json_minor(
                payment.get("gross_amount_minor"),
                "fiat destination-payment gross amount",
                positive=True,
            )
            payment_fee_minor = json_minor(
                payment.get("fee_minor"),
                "fiat destination-payment fee amount",
            )
            payment_net_minor = json_minor(
                payment.get("net_minor"),
                "fiat destination-payment net detail",
                positive=True,
            )
            payment_rate = payment.get("exchange_rate")
            if same_currency:
                payment_rate_matches = payment_rate is None
            else:
                payment_rate_matches = exact_rates_equal(
                    exact_rate_ratio(
                        payment_rate,
                        "fiat destination-payment exchange rate",
                    ),
                    exact_rate_ratio(
                        quote["rates"][output["source_currency"]]["base_rate"],
                        "fiat retained source-currency base rate",
                    ),
                )
            if (
                payment.get("id") != evidence["destination_payment"]
                or payment_source_minor != int(output["source_amount_minor"])
                or payment_source_minor != transfer.get("source_amount_minor")
                or payment.get("source_currency") != output["source_currency"]
                or payment_amount_minor != int(output["destination_amount_minor"])
                or payment_net_minor != payment_amount_minor
                or payment_gross_minor != payment_net_minor + payment_fee_minor
                or payment.get("currency") != output["destination_currency"]
                or not payment_rate_matches
                or payment.get("paid") is not True
                or payment.get("captured") is not True
                or payment.get("source_transfer") != evidence["ref"]
                or not isinstance(payment.get("balance_transaction"), str)
                or not payment["balance_transaction"].startswith("txn_")
            ):
                raise SystemExit("fiat destination-payment readback disagrees with retained evidence")
            provider_source[output["source_currency"]] = (
                provider_source.get(output["source_currency"], 0)
                + int(output["source_amount_minor"])
            )
            provider_destination[output["destination_currency"]] = (
                provider_destination.get(output["destination_currency"], 0)
                + int(output["destination_amount_minor"])
            )

        if len(operator_reports) != operator_count or set(provider_reports) != {
            evidence["ref"]
            for output, evidence in zip(outputs, transfers)
            if output["role"] == "provider"
        }:
            raise SystemExit("fiat Stripe readback is not in one-to-one agreement with outputs")
        reconciliation = d.get("reconciliation")
        expected_reconciliation = {
            "denom": "au_usd",
            "provider_liability_au": value["provider_liability_au"],
            "provider_paid_au": value["provider_paid_au"],
            "operator_fee_liability_au": value["operator_fee_liability_au"],
            "operator_fee_retained_au": value["operator_fee_retained_au"],
            "gross_liability_au": value["gross_liability_au"],
            "gross_paid_au": value["gross_paid_au"],
            "rounding_au": value["rounding_au"],
            "dust_au": value["dust_au"],
            "provider_source_minor_by_currency": {
                key: str(provider_source[key]) for key in sorted(provider_source)
            },
            "provider_destination_minor_by_currency": {
                key: str(provider_destination[key]) for key in sorted(provider_destination)
            },
            "operator_retained_minor_by_currency": {
                key: str(operator_source[key]) for key in sorted(operator_source)
            },
            "operator_fee_mechanism": "retained_platform_balance",
            "provider_output_count": value["provider_count"],
            "verified_transfer_count": value["provider_count"],
            "all_provider_transfers_verified": True,
        }
        if reconciliation != expected_reconciliation:
            raise SystemExit("fiat reconciliation does not exactly match settlement/readback evidence")

    if already is not None:
        outputs, _ = validate_exact(settlement, retained=True)
        if phase == "final" and d.get("submitted") is True:
            raise SystemExit("fiat replay unexpectedly claims a new submit")
        raise SystemExit(0)

    if phase == "plan":
        validate_draft(settlement)
        if d.get("submitted") is True:
            raise SystemExit("fiat plan unexpectedly claims a submit")
        raise SystemExit(0)

    if d.get("nothing_to_settle") is True:
        if no_work is not True:
            raise SystemExit("fiat nothing_to_settle disagrees with no_work")
        validate_draft(settlement)
        raise SystemExit(0)
    if no_work:
        raise SystemExit("fiat no_work disagrees with nothing_to_settle")

    outputs, transfers = validate_exact(settlement)
    validate_preparation_barrier(settlement, outputs)
    state = d.get("settlement_state")
    if not isinstance(state, dict):
        raise SystemExit("fiat final report has no canonical settlement state")
    validate_exact(state, retained=True)
    for key in payload_keys:
        if state.get(key) != settlement.get(key):
            raise SystemExit(f"fiat canonical state disagrees on {key}")
    if d.get("submitted") is not True:
        raise SystemExit("fiat final report does not prove a canonical submit")
    validate_readback(settlement, outputs, transfers)
    raise SystemExit(0)

root = settlement.get("transfer_root")
if not isinstance(root, str) or not re.fullmatch(r"[0-9a-f]{64}", root):
    raise SystemExit(f"{rail} settlement transfer root is invalid")
outputs = settlement.get("outputs")
transfer_key = "stripe_transfers" if rail == "fiat" else "msb_transfers"
transfers = settlement.get(transfer_key)
if not isinstance(outputs, list) or not isinstance(transfers, list):
    raise SystemExit(f"{rail} settlement outputs or transfer evidence is missing")
if already is not None:
    if phase == "final" and d.get("submitted") is True:
        raise SystemExit(f"{rail} replay unexpectedly claims a new submit")
elif no_work:
    if outputs or transfers:
        raise SystemExit(f"{rail} no-work report contains transfer work")
elif phase == "final":
    preparation_ids = settlement.get("preparation_ids")
    effect_ids = settlement.get("external_effect_ids")
    hash_pattern = re.compile(r"^[0-9a-f]{64}$")
    if (
        not isinstance(preparation_ids, list)
        or len(preparation_ids) != len(outputs)
        or len(set(preparation_ids)) != len(outputs)
        or any(not isinstance(item, str) or not hash_pattern.fullmatch(item)
               for item in preparation_ids)
    ):
        raise SystemExit(
            f"{rail} final settlement lacks one unique 32-byte preparation id per output"
        )
    if (
        not isinstance(effect_ids, list)
        or len(effect_ids) != len(outputs)
        or len(set(effect_ids)) != len(outputs)
        or any(not isinstance(item, str) or not hash_pattern.fullmatch(item)
               for item in effect_ids)
    ):
        raise SystemExit(
            f"{rail} final settlement lacks one unique 32-byte external effect id per output"
        )
    preparations = d.get("payout_preparations")
    if not isinstance(preparations, list) or len(preparations) != len(outputs):
        raise SystemExit(
            f"{rail} final report lacks one canonical preparation readback per output"
        )
    by_id = {
        item.get("economic_op_id"): item
        for item in preparations
        if isinstance(item, dict)
    }
    if len(by_id) != len(outputs):
        raise SystemExit(
            f"{rail} canonical preparation readbacks are duplicated or malformed"
        )
    for index, (preparation_id, effect_id) in enumerate(
        zip(preparation_ids, effect_ids)
    ):
        preparation = by_id.get(preparation_id)
        if (
            not isinstance(preparation, dict)
            or preparation.get("type") != "targeted_payout_preparation"
            or preparation.get("rail") != rail
            or preparation.get("epoch") != expected_epoch
            or preparation.get("epoch_apply_hash") != expected_hash
            or preparation.get("output_index") != index
            or preparation.get("external_effect_ids") != [effect_id]
            or preparation.get("consumed") is not False
        ):
            raise SystemExit(
                f"{rail} canonical preparation readback does not bind its output and effect"
            )
    state = d.get("settlement_state")
    if not isinstance(state, dict):
        raise SystemExit(f"{rail} final report has no canonical settlement state")
    for key in (
        "op", "rail", "epoch", "epoch_apply_hash", "preparation_ids",
        "external_effect_ids", "transfer_root", "outputs", transfer_key,
    ):
        if state.get(key) != settlement.get(key):
            raise SystemExit(f"{rail} canonical state disagrees on {key}")
    if d.get("submitted") is not True or len(transfers) != len(outputs) or not transfers:
        raise SystemExit(f"{rail} final transfer evidence is incomplete")

if phase == "final" and already is None and not no_work:
    outer_outputs = d.get("msb_outputs")
    outer = d.get("msb_transfers")
    if not isinstance(outer_outputs, list) or len(outer_outputs) != len(outputs):
        raise SystemExit("TNK MSB outputs do not cover settlement outputs")
    if not isinstance(outer, list) or len(outer) != len(outputs):
        raise SystemExit("TNK MSB transfer journals do not cover settlement outputs")
    by_index = {item.get("output_index"): item for item in outer}
    for index, evidence in enumerate(transfers):
        report = by_index.get(index)
        transfer = report.get("transfer") if isinstance(report, dict) else None
        if (
            not isinstance(transfer, dict)
            or transfer.get("tx_hash") != evidence.get("tx_hash")
            or transfer.get("from") != evidence.get("from")
            or transfer.get("to") != evidence.get("to")
            or transfer.get("confirmed_length") != evidence.get("confirmed_length")
            or transfer.get("observed_signed_length") != evidence.get("observed_signed_length")
        ):
            raise SystemExit("TNK MSB transfer report disagrees with retained evidence")
PY
}

validate_tap_processed() {
  local report="$1" bundle="$2"
  python3 - "$report" "$bundle" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, re, sys
report_path, bundle_path, expected_epoch, expected_hash = sys.argv[1:]
expected_epoch = int(expected_epoch)
d = json.load(open(report_path))
bundle_raw = open(bundle_path, "rb").read()
bundle = json.loads(bundle_raw)
if (
    d.get("rail") != "tap"
    or d.get("epoch") != expected_epoch
    or d.get("epoch_apply_hash") != expected_hash
    or d.get("bundle_sha256") != hashlib.sha256(bundle_raw).hexdigest()
):
    raise SystemExit("TAP report is stale or not bound to its exact spool bundle")
if (
    bundle.get("rail") != "tap"
    or bundle.get("epoch") != expected_epoch
    or bundle.get("epoch_apply_hash") != expected_hash
):
    raise SystemExit("TAP spool bundle is stale or rail-contaminated")
root = d.get("root")
if not isinstance(root, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", root):
    raise SystemExit("TAP report root evidence is invalid")
if d.get("blocked") is True or d.get("root_confirmed") is not True:
    raise SystemExit("TAP root is not confirmed")
tx = d.get("execution_tx") or d.get("proposal_tx")
if d.get("root_already_posted") is not True:
    if d.get("posted") is not True or not isinstance(tx, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", tx):
        raise SystemExit("TAP report has no confirmed root transaction evidence")

def stable_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))

def canonical_uint(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"(0|[1-9][0-9]*)", value):
        raise SystemExit(f"{label} is not a canonical unsigned integer")
    return int(value)

checkpoint = d.get("tap_settlement_checkpoint")
checkpoint_keys = {
    "op", "epoch", "at", "rail", "chain_id", "token_address",
    "pool_address", "payment_config_ver", "epoch_apply_hash",
    "tap_rate_lock", "root", "root_confirmed", "proposal_tx",
    "proposal_block_number", "proposal_block_hash", "execution_tx",
    "execution_status", "execution_block_number", "execution_block_hash",
    "finalized_block_number", "confirmation_depth", "confirmation_policy",
    "cumulative_spent_wei", "provider_count", "provider_paid_au",
    "provider_tap_wei", "entries", "outputs",
}
if (
    not isinstance(checkpoint, dict)
    or set(checkpoint) != checkpoint_keys
    or checkpoint.get("op") != "settle_targeted_tap"
    or checkpoint.get("rail") != "tap"
    or checkpoint.get("epoch") != expected_epoch
    or checkpoint.get("epoch_apply_hash") != expected_hash
    or checkpoint.get("root") != d.get("root", "").lower()
    or checkpoint.get("root_confirmed") is not True
):
    raise SystemExit("TAP canonical settlement checkpoint identity is invalid")
rate_lock = d.get("tap_rate_lock")
if (
    not isinstance(rate_lock, dict)
    or checkpoint.get("tap_rate_lock") != rate_lock
    or checkpoint.get("chain_id") != rate_lock.get("chain_id")
    or checkpoint.get("token_address") != str(rate_lock.get("token_address", "")).lower()
    or checkpoint.get("pool_address") != str(rate_lock.get("pool_address", "")).lower()
    or checkpoint.get("payment_config_ver") != rate_lock.get("payment_config_ver")
):
    raise SystemExit("TAP canonical settlement checkpoint rate lock is invalid")
confirmation = d.get("root_confirmation")
proposal = confirmation.get("proposal") if isinstance(confirmation, dict) else None
execution = confirmation.get("execution") if isinstance(confirmation, dict) else None
if (
    not isinstance(confirmation, dict)
    or confirmation.get("confirmed") is not True
    or confirmation.get("onchain_epoch") != expected_epoch
    or confirmation.get("onchain_root") != checkpoint.get("root")
    or confirmation.get("onchain_cumulative_spent_wei") !=
        checkpoint.get("cumulative_spent_wei")
    or not isinstance(proposal, dict)
    or not isinstance(execution, dict)
    or checkpoint.get("proposal_tx") != proposal.get("tx_hash")
    or checkpoint.get("proposal_block_number") != proposal.get("block_number")
    or checkpoint.get("proposal_block_hash") != proposal.get("block_hash")
    or checkpoint.get("execution_tx") != execution.get("tx_hash")
    or checkpoint.get("execution_status") != execution.get("status")
    or checkpoint.get("execution_block_number") != execution.get("block_number")
    or checkpoint.get("execution_block_hash") != execution.get("block_hash")
    or checkpoint.get("finalized_block_number") != confirmation.get("finalized_block_number")
    or checkpoint.get("confirmation_depth") != confirmation.get("confirmation_depth")
    or checkpoint.get("confirmation_policy") != confirmation.get("confirmation_policy")
    or not re.fullmatch(r"0x[0-9a-f]{64}", str(checkpoint.get("proposal_tx", "")))
    or not re.fullmatch(r"0x[0-9a-f]{64}", str(checkpoint.get("execution_tx", "")))
    or checkpoint.get("execution_status") != 1
    or checkpoint.get("confirmation_depth") !=
        checkpoint.get("finalized_block_number") - checkpoint.get("execution_block_number")
):
    raise SystemExit("TAP canonical settlement checkpoint root confirmation is invalid")
entries = checkpoint.get("entries")
if not isinstance(entries, list) or not entries:
    raise SystemExit("TAP canonical settlement checkpoint entries are invalid")
entry_map = {}
for entry in entries:
    if (
        not isinstance(entry, dict)
        or set(entry) != {"account", "cumulative_wei"}
        or not re.fullmatch(r"0x[0-9a-f]{40}", str(entry.get("account", "")))
    ):
        raise SystemExit("TAP canonical settlement checkpoint entry shape is invalid")
    account = entry["account"]
    if account in entry_map:
        raise SystemExit("TAP canonical settlement checkpoint entry is duplicated")
    entry_map[account] = canonical_uint(
        entry.get("cumulative_wei"),
        "TAP cumulative entry",
    )
if entries != sorted(entries, key=lambda entry: entry["account"]):
    raise SystemExit("TAP canonical settlement checkpoint entries are not sorted")
if str(sum(entry_map.values())) != checkpoint.get("cumulative_spent_wei"):
    raise SystemExit("TAP canonical cumulative settlement total is invalid")
outputs = checkpoint.get("outputs")
if (
    not isinstance(outputs, list)
    or not outputs
    or checkpoint.get("provider_count") != len(outputs)
):
    raise SystemExit("TAP canonical settlement checkpoint outputs are invalid")
seen = set()
net_au = 0
tap_wei = 0
for output in outputs:
    if not isinstance(output, dict) or sorted(output) != [
        "cumulative_claim_wei",
        "paid_au",
        "payout_revision",
        "provider",
        "tap_wei",
        "to",
    ]:
        raise SystemExit("TAP canonical provider checkpoint output shape is invalid")
    identity = (output.get("provider"), output.get("payout_revision"))
    if (
        not re.fullmatch(r"[0-9a-f]{64}", str(identity[0] or ""))
        or not re.fullmatch(r"[0-9a-f]{64}", str(identity[1] or ""))
        or not re.fullmatch(r"0x[0-9a-f]{40}", str(output.get("to", "")))
        or identity in seen
    ):
        raise SystemExit("TAP canonical provider checkpoint identity is invalid")
    seen.add(identity)
    paid_now = canonical_uint(output.get("paid_au"), "TAP paid_au")
    output_wei = canonical_uint(output.get("tap_wei"), "TAP output tap_wei")
    cumulative = canonical_uint(
        output.get("cumulative_claim_wei"),
        "TAP cumulative claim",
    )
    if (
        paid_now <= 0
        or output_wei <= 0
        or entry_map.get(output.get("to")) != cumulative
    ):
        raise SystemExit("TAP canonical provider checkpoint amount is invalid")
    net_au += paid_now
    tap_wei += output_wei
if (
    str(net_au) != checkpoint.get("provider_paid_au")
    or str(tap_wei) != checkpoint.get("provider_tap_wei")
):
    raise SystemExit("TAP canonical settlement checkpoint totals do not match outputs")
for field in ("operator_fee", "burn"):
    record = d.get(field)
    if not isinstance(record, dict):
        raise SystemExit(f"TAP {field} evidence is missing")
    predicted = record.get("predicted_claimable_wei")
    remaining = record.get("remaining_claimable_wei")
    if (
        record.get("completed") is not True
        or not isinstance(predicted, str)
        or not predicted.isdigit()
        or remaining != "0"
    ):
        raise SystemExit(f"TAP {field} is incomplete")
    if predicted != "0":
        transfer_tx = record.get("tx")
        if (
            record.get("auto_sent") is not True
            or not isinstance(transfer_tx, str)
            or not re.fullmatch(r"0x[0-9a-fA-F]{64}", transfer_tx)
        ):
            raise SystemExit(f"TAP {field} transfer evidence is missing")
PY
}

validate_tap_no_work() {
  local report="$1" bundle="$2"
  python3 - "$report" "$bundle" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, re, sys
report_path, bundle_path, expected_epoch, expected_hash = sys.argv[1:]
expected_epoch = int(expected_epoch)
bundle_raw = open(bundle_path, "rb").read()
bundle = json.loads(bundle_raw)
report = json.load(open(report_path))

def uint(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"(0|[1-9][0-9]*)", value):
        raise SystemExit(f"{label} is not a canonical unsigned integer")
    return int(value)

receipts = bundle.get("receipts")
if (
    bundle.get("rail") != "tap"
    or bundle.get("epoch") != expected_epoch
    or bundle.get("epoch_apply_hash") != expected_hash
    or not isinstance(receipts, list)
    or not receipts
):
    raise SystemExit("TAP carry bundle identity is invalid")
if (
    report.get("rail") != "tap"
    or report.get("epoch") != expected_epoch
    or report.get("epoch_apply_hash") != expected_hash
    or report.get("bundle_sha256") != hashlib.sha256(bundle_raw).hexdigest()
    or report.get("status") != "no_work"
    or report.get("outcome") != "carry"
    or report.get("blocked") is True
    or report.get("tap_settlement_checkpoint") is not None
    or report.get("checkpoint_outputs") != []
    or report.get("spent_au") != "0"
    or report.get("receipt_count") != len(receipts)
):
    raise SystemExit("TAP carry report is stale, incomplete, or has payable output")

held_count = report.get("held_receipt_count")
threshold_count = report.get("threshold_held_provider_count")
if (
    not isinstance(held_count, int)
    or isinstance(held_count, bool)
    or held_count < 0
    or not isinstance(threshold_count, int)
    or isinstance(threshold_count, bool)
    or threshold_count < 0
    or held_count + threshold_count < 1
):
    raise SystemExit("TAP carry report has no explicit held or below-threshold work")
held_au = uint(report.get("held_au"), "TAP held_au")
threshold_au = uint(report.get("threshold_held_au"), "TAP threshold_held_au")
payout_min_au = uint(report.get("payout_min_au"), "TAP payout_min_au")
if held_au < threshold_au:
    raise SystemExit("TAP carry totals regress below-threshold liability")
if held_count == 0 and held_au != threshold_au:
    raise SystemExit("TAP carry report omits held-liability classification")

deferred = report.get("canonical_deferred_liabilities")
if not isinstance(deferred, list) or len(deferred) != threshold_count:
    raise SystemExit("TAP carry report does not enumerate each below-threshold liability")
rate_lock = report.get("tap_rate_lock")
rate = uint(
    rate_lock.get("tap_usd_au") if isinstance(rate_lock, dict) else None,
    "TAP locked rate",
)
seen = set()
for item in deferred:
    if not isinstance(item, dict) or set(item) != {
        "provider", "payout_revision", "to", "payable_au", "reason"
    }:
        raise SystemExit("TAP deferred liability has an invalid exact shape")
    identity = (item.get("provider"), item.get("payout_revision"), item.get("to"))
    if (
        not re.fullmatch(r"[0-9a-f]{64}", str(identity[0] or ""))
        or not re.fullmatch(r"[0-9a-f]{64}", str(identity[1] or ""))
        or not re.fullmatch(r"0x[0-9a-f]{40}", str(identity[2] or ""))
        or identity in seen
    ):
        raise SystemExit("TAP deferred liability identity is invalid or duplicated")
    seen.add(identity)
    payable = uint(item.get("payable_au"), "TAP deferred payable_au")
    if payable <= 0:
        raise SystemExit("TAP deferred liability must be positive")
    if item.get("reason") == "below_payout_minimum":
        if payable >= payout_min_au:
            raise SystemExit("TAP deferred liability is not below payout_min_au")
    elif item.get("reason") == "below_tap_wei_precision":
        if payable * 10**18 // rate != 0:
            raise SystemExit("TAP deferred liability is representable in TAP wei")
    else:
        raise SystemExit("TAP deferred liability reason is not canonical")

reason = report.get("reason")
if reason not in {
    "provider earnings await challenge or holdback maturity",
    "provider earnings are below payout minimum",
}:
    raise SystemExit("TAP carry report reason is not canonical")
PY
}

complete_tap_processed() {
  local report="$1" bundle="$2" status
  status="$(printf '%s' "$(cat "$report")" | json_field status)"
  if [[ "$status" == "no_work" ]]; then
    validate_tap_no_work "$report" "$bundle" || return 1
    mark_rail tap carry "$report"
    echo "tap: canonical held/below-threshold liabilities carried forward"
    return 0
  fi
  [[ -z "$status" ]] || {
    echo "tap: processed report has unsupported status $status" >&2
    return 1
  }
  validate_tap_processed "$report" "$bundle" || return 1
  reconcile_tap_contract_checkpoint "$report"
}

validate_tap_cli_report() {
  local report="$1" checkpoint="$2" expected_submitted="$3" expected_sim="$4"
  python3 - "$report" "$checkpoint" "$expected_submitted" "$expected_sim" <<'PY'
import json, re, sys
report = json.load(open(sys.argv[1]))
checkpoint = json.load(open(sys.argv[2]))
expected_submitted = sys.argv[3] == "true"
expected_sim = sys.argv[4] == "true"
feature = report.get("feature")
key = feature.get("key") if isinstance(feature, dict) else None
if (
    report.get("ok") is not True
    or report.get("feature_type") != "settleTargetedTap"
    or report.get("submitted") is not expected_submitted
    or (report.get("sim") is True) is not expected_sim
    or not isinstance(feature, dict)
    or feature.get("feature") != "mayhem"
    or feature.get("value") != checkpoint
    or not isinstance(key, str)
    or not re.fullmatch(
        rf"settle/targeted/tap/{checkpoint['epoch']}/[0-9a-f]{{64}}",
        key,
    )
):
    raise SystemExit("TAP admin CLI report does not bind the exact checkpoint")
PY
}

tap_contract_record_status() {
  local checkpoint="$1" state_file="$2"
  python3 - "$checkpoint" "$state_file" <<'PY'
import json, re, sys
checkpoint = json.load(open(sys.argv[1]))
record = json.load(open(sys.argv[2])).get("value")
if record is None:
    raise SystemExit(2)
if not isinstance(record, dict):
    raise SystemExit("canonical TAP settlement state is malformed")
expected_keys = set(checkpoint) | {"type", "settled_by", "settled_by_role"}
if (
    set(record) != expected_keys
    or record.get("type") != "targeted_tap_settlement"
    or record.get("settled_by_role") != "admin"
    or not re.fullmatch(r"[0-9a-f]{64}", str(record.get("settled_by", "")))
    or any(record.get(key) != value for key, value in checkpoint.items())
):
    raise SystemExit("canonical TAP settlement state conflicts with retained checkpoint")
PY
}

reconcile_tap_contract_checkpoint() {
  local processed_report="$1"
  local checkpoint="$work_dir/tap-contract-checkpoint.json"
  local sim_report="$work_dir/tap-contract-sim.json"
  local submit_report="$work_dir/tap-contract-submit.json"
  local canonical_record="$work_dir/tap-contract-record.json"
  local reconciliation="$work_dir/tap-contract-reconciliation.json"
  local attempt state_status

  if ! python3 - "$processed_report" "$checkpoint.tmp" <<'PY'
import json, os, sys
report = json.load(open(sys.argv[1]))
checkpoint = report.get("tap_settlement_checkpoint")
if not isinstance(checkpoint, dict):
    raise SystemExit("processed TAP report has no canonical settlement checkpoint")
with open(sys.argv[2], "w") as out:
    json.dump(checkpoint, out, indent=2)
    out.write("\n")
    out.flush()
    os.fsync(out.fileno())
PY
  then
    rm -f "$checkpoint.tmp"
    return 1
  fi
  if [[ -f "$checkpoint" ]] && ! cmp -s "$checkpoint" "$checkpoint.tmp"; then
    rm -f "$checkpoint.tmp"
    echo "tap: retained contract checkpoint conflicts with processed roller evidence" >&2
    return 1
  fi
  mv "$checkpoint.tmp" "$checkpoint"

  local -a args=(
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --peer-store-name "$ADMIN_STORE"
    --checkpoint-file "$checkpoint"
    --json
  )
  if ! "$MAYHEM_BIN" admin tap-settlement "${args[@]}" \
    --submit --sim >"$sim_report.tmp"; then
    rm -f "$sim_report.tmp"
    echo "tap: canonical checkpoint simulation failed" >&2
    return 1
  fi
  mv "$sim_report.tmp" "$sim_report"
  validate_tap_cli_report "$sim_report" "$checkpoint" false true || return 1

  if ! "$MAYHEM_BIN" admin tap-settlement "${args[@]}" \
    --submit >"$submit_report.tmp"; then
    rm -f "$submit_report.tmp"
    echo "tap: canonical checkpoint submission failed" >&2
    return 1
  fi
  mv "$submit_report.tmp" "$submit_report"
  validate_tap_cli_report "$submit_report" "$checkpoint" true false || return 1

  for ((attempt = 1; attempt <= TAP_CONTRACT_WAIT_ATTEMPTS; attempt += 1)); do
    if ! curl -sf -m 10 \
      "$RPC_URL/state?key=settle/targeted/tap/$applied_epoch" \
      >"$canonical_record.tmp"; then
      rm -f "$canonical_record.tmp"
      echo "tap: canonical settlement readback failed" >&2
      return 1
    fi
    mv "$canonical_record.tmp" "$canonical_record"
    state_status=0
    tap_contract_record_status "$checkpoint" "$canonical_record" || state_status=$?
    if (( state_status == 0 )); then
      break
    elif (( state_status != 2 )); then
      return 1
    fi
    if (( attempt == TAP_CONTRACT_WAIT_ATTEMPTS )); then
      echo "tap: canonical settlement record did not become visible" >&2
      return 1
    fi
    sleep "$TAP_CONTRACT_WAIT_SECONDS"
  done

  if [[ "${MAYHEM_PAYOUT_TEST_CRASH_AFTER_TAP_CONTRACT:-0}" == "1" ]]; then
    [[ "${MAYHEM_PAYOUT_TEST_MODE:-0}" == "1" ]] || {
      echo "abort: TAP contract crash hook is test-mode only" >&2
      return 1
    }
    echo "tap: simulated crash after canonical settlement and before completion marker" >&2
    return 98
  fi

  python3 - "$reconciliation.tmp" "$processed_report" "$checkpoint" \
    "$sim_report" "$submit_report" "$canonical_record" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, os, sys
target, processed, checkpoint, sim, submit, record, epoch, apply_hash = sys.argv[1:]
def digest(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()
with open(target, "w") as out:
    json.dump({
        "schema_version": 1,
        "type": "canonical_tap_settlement_reconciliation",
        "epoch": int(epoch),
        "epoch_apply_hash": apply_hash,
        "processed_report_sha256": digest(processed),
        "checkpoint_sha256": digest(checkpoint),
        "simulation_sha256": digest(sim),
        "submission_sha256": digest(submit),
        "canonical_record_sha256": digest(record),
    }, out, indent=2)
    out.write("\n")
    out.flush()
    os.fsync(out.fileno())
PY
  mv "$reconciliation.tmp" "$reconciliation"
  mark_rail tap settled "$reconciliation"
  echo "tap: Ethereum execution and canonical Intercom settlement reconciled"
}

settle_fiat() {
  [[ -f "$work_dir/fiat.complete" ]] && {
    validate_rail_marker fiat || return 1
    echo "fiat: already reconciled for epoch $applied_epoch"
    return 0
  }

  local attempt attempt_result=0 final_tmp final_file error_file
  attempt="$(next_attempt fiat)" || attempt_result=$?
  [[ "$attempt_result" == "0" ]] || return "$attempt_result"
  final_tmp="$work_dir/fiat-settlement.json.tmp"
  final_file="$work_dir/fiat-settlement.json"
  error_file="$work_dir/fiat-attempt-$attempt.stderr.log"
  local -a args=(
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --peer-store-name "$ADMIN_STORE"
    --epoch "$applied_epoch"
    --at "$settlement_at"
    --operator-stripe-account "$FIAT_OPERATOR_ACCOUNT"
    --stripe-transfer-max-attempts "$FIAT_TRANSFER_MAX_ATTEMPTS"
    --stripe-transfer-retry-ms "$FIAT_TRANSFER_RETRY_MS"
    --json
  )
  [[ -n "$FIAT_STRIPE_ENV_FILE" ]] && args+=(--stripe-env-file "$FIAT_STRIPE_ENV_FILE")

  if ! "$MAYHEM_BIN" admin fiat-settlement "${args[@]}" \
    --submit-transfer --submit >"$final_tmp" 2>"$error_file"; then
    rm -f "$final_tmp"
    echo "fiat: transfer/evidence submit failed on attempt $attempt (see $error_file)" >&2
    return 1
  fi
  mv "$final_tmp" "$final_file"
  if ! validate_settlement_report "$final_file" fiat final; then
    echo "fiat: final report did not prove exact settlement (see $final_file)" >&2
    return 1
  fi
  if python3 - "$final_file" <<'PY'
import json, sys
raise SystemExit(0 if json.load(open(sys.argv[1])).get("already_settled") is not None else 1)
PY
  then
    mark_rail fiat already_settled "$final_file"
    echo "fiat: canonical settlement evidence already exists"
    return 0
  fi
  local outcome
  outcome="$(python3 - "$final_file" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
print(report["settlement"]["outcome"])
PY
)"
  case "$outcome" in
    payouts)
      mark_rail fiat settled "$final_file"
      echo "fiat: Stripe transfers and canonical close reconciled"
      ;;
    carry)
      mark_rail fiat carry "$final_file"
      echo "fiat: below-threshold liabilities carried and canonically closed"
      ;;
    no_work)
      mark_rail fiat no_work "$final_file"
      echo "fiat: empty epoch plan canonically closed"
      ;;
    *)
      echo "fiat: final report has an unsupported canonical outcome" >&2
      return 1
      ;;
  esac
}

settle_tnk() {
  [[ -f "$work_dir/tnk.complete" ]] && {
    validate_rail_marker tnk || return 1
    echo "tnk: already reconciled for epoch $applied_epoch"
    return 0
  }

  local attempt attempt_result=0 plan_tmp plan_file error_file
  attempt="$(next_attempt tnk)" || attempt_result=$?
  [[ "$attempt_result" == "0" ]] || return "$attempt_result"
  plan_tmp="$work_dir/tnk-plan.json.tmp"
  plan_file="$work_dir/tnk-plan.json"
  error_file="$work_dir/tnk-attempt-$attempt.stderr.log"
  local -a args=(
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --peer-store-name "$ADMIN_STORE"
    --epoch "$applied_epoch"
    --at "$settlement_at"
    --msb-transfer-timeout-seconds "$TNK_TRANSFER_TIMEOUT_SECONDS"
    --msb-transfer-max-retries "$TNK_TRANSFER_MAX_RETRIES"
    --json
  )

  if ! "$MAYHEM_BIN" admin tnk-settlement "${args[@]}" >"$plan_tmp" 2>"$error_file"; then
    rm -f "$plan_tmp"
    echo "tnk: planning failed on attempt $attempt (see $error_file)" >&2
    return 1
  fi
  mv "$plan_tmp" "$plan_file"
  if ! validate_settlement_report "$plan_file" tnk plan; then
    echo "tnk: plan is not bound to the current rail/epoch/apply hash (see $plan_file)" >&2
    return 1
  fi

  local plan_state tnk_ok tnk_already tnk_blocking
  plan_state="$(python3 - "$plan_file" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
blocking = sum(1 for item in d.get("skipped_providers", []) if item.get("blocking", True))
print(
    str(bool(d.get("ok"))).lower(),
    str(d.get("already_settled") is not None).lower(),
    blocking,
)
PY
)"
  read -r tnk_ok tnk_already tnk_blocking <<<"$plan_state"
  if [[ "$tnk_ok" != "true" || "$tnk_blocking" != "0" ]]; then
    echo "tnk: plan has blocking payout errors; no MSB transfer attempted (see $plan_file)" >&2
    return 1
  fi
  if [[ "$tnk_already" == "true" ]]; then
    mark_rail tnk already_settled "$plan_file"
    echo "tnk: canonical settlement evidence already exists"
    return 0
  fi

  local final_tmp="$work_dir/tnk-settlement.json.tmp"
  local final_file="$work_dir/tnk-settlement.json"
  if ! "$MAYHEM_BIN" admin tnk-settlement "${args[@]}" \
    --submit-transfer --submit >"$final_tmp" 2>"$error_file"; then
    rm -f "$final_tmp"
    echo "tnk: transfer/evidence submit failed on attempt $attempt (see $error_file)" >&2
    return 1
  fi
  mv "$final_tmp" "$final_file"
  if ! validate_settlement_report "$final_file" tnk final; then
    echo "tnk: final report did not prove exact settlement (see $final_file)" >&2
    return 1
  fi
  local outcome
  outcome="$(python3 - "$final_file" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
print(report["settlement"]["outcome"])
PY
)"
  case "$outcome" in
    payouts)
      mark_rail tnk settled "$final_file"
      echo "tnk: MSB transfers and canonical close reconciled"
      ;;
    carry)
      mark_rail tnk carry "$final_file"
      echo "tnk: below-threshold liabilities carried and canonically closed"
      ;;
    no_work)
      mark_rail tnk no_work "$final_file"
      echo "tnk: empty epoch plan canonically closed"
      ;;
    *)
      echo "tnk: final report has an unsupported canonical outcome" >&2
      return 1
      ;;
  esac
}

produce_tap_work() {
  [[ -f "$work_dir/tap.complete" ]] && {
    validate_rail_marker tap || return 1
    echo "tap: settlement already reconciled for epoch $applied_epoch"
    return 0
  }

  local attempt attempt_result=0 bundle temp_bundle receipt_count name existing
  local processed_report spool_state spool_path spool_count
  name="epoch-$applied_epoch-$apply_hash.receipts.json"
  processed_report="$TAP_SPOOL/processed/${name%.receipts.json}.settlement.json"

  tap_spool_state() {
    spool_state=""
    spool_path=""
    spool_count=0
    local state candidate
    for state in ready working processed failed; do
      candidate="$TAP_SPOOL/$state/$name"
      [[ -f "$candidate" ]] || continue
      spool_count=$((spool_count + 1))
      spool_state="$state"
      spool_path="$candidate"
    done
    if (( spool_count > 1 )); then
      echo "tap: duplicate spool item exists in more than one lifecycle state" >&2
      return 1
    fi
  }

  validate_tap_produced() {
    local current_bundle="$1"
    python3 - "$work_dir/tap.produced" "$current_bundle" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, sys
marker_path, bundle_path, expected_epoch, expected_hash = sys.argv[1:]
d = json.load(open(marker_path))
if (
    d.get("schema_version") != 1
    or d.get("rail") != "tap"
    or d.get("status") != "produced"
    or d.get("epoch") != int(expected_epoch)
    or d.get("epoch_apply_hash") != expected_hash
    or d.get("evidence_sha256") != hashlib.sha256(open(bundle_path, "rb").read()).hexdigest()
):
    raise SystemExit("TAP produced marker is stale or malformed")
PY
  }

  if [[ -f "$work_dir/tap.produced" ]]; then
    tap_spool_state || return 1
    if (( spool_count == 0 )); then
      echo "tap: produced spool item is missing from every lifecycle state" >&2
      return 1
    fi
    validate_tap_produced "$spool_path" || return 1
    case "$spool_state" in
      ready|working)
        echo "tap: settlement worker owns $spool_path in $spool_state state"
        return 2
        ;;
      failed)
        echo "tap: spool item is in failed state at $spool_path" >&2
        return 1
        ;;
      processed)
        if [[ ! -f "$processed_report" ]] || \
          ! complete_tap_processed "$processed_report" "$spool_path"; then
          echo "tap: processed spool item lacks exact settlement evidence at $processed_report" >&2
          return 1
        fi
        return 0
        ;;
    esac
  fi

  attempt="unpublished"
  bundle="$epoch_dir/epoch-bundle.json"
  if [[ ! -f "$bundle" ]]; then
    local empty_seal
    empty_seal="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/seal/$applied_epoch")"
    printf '%s\n' "$empty_seal" >"$work_dir/tap-empty-seal.json.tmp"
    mv "$work_dir/tap-empty-seal.json.tmp" "$work_dir/tap-empty-seal.json"
    if ! python3 - "$work_dir/tap-empty-seal.json" "$applied_epoch" "$apply_hash" <<'PY'
import json, sys
record = json.load(open(sys.argv[1])).get("value") or {}
zero_totals = {
    "debited_au": "0",
    "earned_au": "0",
    "fee_au": "0",
    "burn_au": "0",
}
if (
    record.get("type") != "epoch_empty_seal"
    or record.get("epoch") != int(sys.argv[2])
    or str(record.get("seal_hash", "")).lower() != sys.argv[3]
    or record.get("totals") != zero_totals
):
    raise SystemExit("missing receipt bundle is not backed by the canonical empty-epoch seal")
PY
    then
      echo "tap: finalized non-empty epoch is missing its retained receipt bundle" >&2
      return 1
    fi
    mark_rail tap no_work "$work_dir/tap-empty-seal.json"
    echo "tap: canonical empty-epoch seal proves there is no TAP work"
    return 0
  fi

  temp_bundle="$work_dir/tap-receipts.json.tmp"
  if ! receipt_count="$(python3 - "$bundle" "$temp_bundle" "$applied_epoch" "$apply_hash" <<'PY'
import copy, json, sys
source, target, expected_epoch, apply_hash = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
bundle = json.load(open(source))
if bundle.get("epoch") != expected_epoch:
    raise SystemExit("finalized receipt bundle epoch does not match canonical applied epoch")
receipts = []
for entry in bundle.get("receipts", []):
    if not isinstance(entry, dict):
        raise SystemExit("receipt entry must be an object")
    receipt = entry.get("receipt")
    body = receipt.get("body") if isinstance(receipt, dict) else None
    if not isinstance(body, dict):
        raise SystemExit("signed receipt body must be an object")
    outer = entry.get("rail")
    inner = body.get("rail")
    if not isinstance(outer, str) or not isinstance(inner, str):
        raise SystemExit("receipt outer rail and signed body rail are required")
    if outer != outer.lower() or inner != inner.lower() or outer != inner:
        raise SystemExit("receipt outer rail does not match signed receipt rail")
    rail = inner
    if rail not in {"fiat", "tap", "tnk"}:
        raise SystemExit("signed receipt rail is unsupported")
    if rail == "tap":
        item = copy.deepcopy(entry)
        item["receipt_epoch"] = expected_epoch
        receipts.append(item)
out = {key: value for key, value in bundle.items() if key != "receipts"}
out["rail"] = "tap"
out["epoch_apply_hash"] = apply_hash
out["receipts"] = receipts
with open(target, "w") as handle:
    json.dump(out, handle, indent=2)
    handle.write("\n")
print(len(receipts))
PY
)"; then
    rm -f "$temp_bundle"
    echo "tap: failed to derive a rail-isolated spool bundle on attempt $attempt" >&2
    return 1
  fi
  [[ "$receipt_count" =~ ^(0|[1-9][0-9]*)$ ]] || {
    rm -f "$temp_bundle"
    echo "tap: derived receipt count is not a canonical non-negative integer" >&2
    return 1
  }

  if [[ "$receipt_count" == "0" ]]; then
    rm -f "$temp_bundle"
    mark_rail tap no_work "$bundle"
    echo "tap: finalized bundle has no TAP receipts"
    return 0
  fi

  tap_spool_state || {
    rm -f "$temp_bundle"
    return 1
  }
  if (( spool_count == 1 )); then
    existing="$spool_path"
    if ! cmp -s "$temp_bundle" "$spool_path"; then
      rm -f "$temp_bundle"
      echo "tap: existing spool item $existing differs from canonical epoch bundle" >&2
      return 1
    fi
    rm -f "$temp_bundle"
    mark_tap_produced "$existing"
    case "$spool_state" in
      failed)
        echo "tap: spool item is in failed state at $existing" >&2
        return 1
        ;;
      processed)
        if [[ ! -f "$processed_report" ]] || \
          ! complete_tap_processed "$processed_report" "$existing"; then
        echo "tap: processed spool item lacks complete settlement evidence at $processed_report" >&2
        return 1
        fi
        return 0
        ;;
      ready|working)
        echo "tap: canonical spool item already exists in $spool_state state at $existing"
        return 2
        ;;
    esac
  fi

  attempt="$(next_attempt tap)" || attempt_result=$?
  [[ "$attempt_result" == "0" ]] || {
    rm -f "$temp_bundle"
    return "$attempt_result"
  }
  mv "$temp_bundle" "$TAP_SPOOL/ready/$name"
  if [[ "${MAYHEM_PAYOUT_TEST_CRASH_AFTER_TAP_QUEUE:-0}" == "1" ]]; then
    [[ "${MAYHEM_PAYOUT_TEST_MODE:-0}" == "1" ]] || {
      echo "abort: TAP crash hook is test-mode only" >&2
      return 1
    }
    echo "tap: simulated crash after atomic queue publication" >&2
    return 97
  fi
  mark_tap_produced "$TAP_SPOOL/ready/$name"
  echo "tap: queued $receipt_count TAP receipt(s) for the simulate-first worker"
  return 2
}

resume_prior_rail_work() {
  local rail="$1"
  [[ "${MAYHEM_PAYOUT_RESUME_PRIOR:-0}" != "1" ]] || return 0

  local prior_dir prior_epoch resume_result
  local resume_list="$work_dir/$rail-prior-payout-work.list"
  if ! python3 - "$STATE_DIR/payout" "$work_dir" "$canonical_applied_epoch" "$rail" \
    >"$resume_list.tmp" <<'PY'
import os, re, sys
root, current, canonical_epoch, rail = (
    sys.argv[1],
    os.path.abspath(sys.argv[2]),
    int(sys.argv[3]),
    sys.argv[4],
)
pattern = re.compile(r"epoch-([1-9][0-9]*)-([0-9a-f]{64})")
pending = []
for name in os.listdir(root):
    match = pattern.fullmatch(name)
    if not match:
        continue
    path = os.path.abspath(os.path.join(root, name))
    if path == current:
        continue
    if os.path.islink(path) or not os.path.isdir(path):
        raise SystemExit(f"payout work entry is not a real directory: {path}")
    epoch = int(match.group(1))
    if epoch > canonical_epoch:
        raise SystemExit(f"future payout work directory is unsafe: {path}")
    if not os.path.isfile(os.path.join(path, f"{rail}.complete")):
        pending.append((epoch, match.group(2), path))
for _, _, path in sorted(pending):
    sys.stdout.buffer.write(os.fsencode(path) + b"\0")
PY
  then
    rm -f "$resume_list.tmp"
    echo "abort: could not enumerate retained payout operations safely" >&2
    return 1
  fi
  mv "$resume_list.tmp" "$resume_list"

  while IFS= read -r -d '' prior_dir; do
    prior_epoch="${prior_dir##*/epoch-}"
    prior_epoch="${prior_epoch%%-*}"
    echo "recovery: resuming unresolved $rail payout work for epoch $prior_epoch before new $rail liabilities"
    resume_result=0
    MAYHEM_PAYOUT_RESUME_PRIOR=1 \
      MAYHEM_PAYOUT_RESUME_RAIL="$rail" \
      MAYHEM_PAYOUT_RESUME_WORK_DIR="$prior_dir" \
      bash "$0" "$prior_epoch" || resume_result=$?
    if (( resume_result != 0 )); then
      echo "abort: prior payout work remains failed for $rail at $prior_dir" >&2
      return 1
    fi
    if [[ ! -f "$prior_dir/$rail.complete" ]]; then
      echo "recovery: prior $rail payout work remains pending at $prior_dir; current $rail liabilities deferred"
      return 2
    fi
  done <"$resume_list"
  rm -f "$resume_list"
}

resume_only_rail="${MAYHEM_PAYOUT_RESUME_RAIL:-}"
if [[ -n "$resume_only_rail" ]]; then
  [[ "${MAYHEM_PAYOUT_RESUME_PRIOR:-0}" == "1" && -n "$resume_work_dir" ]] || {
    echo "abort: single-rail payout resume is internal-only" >&2
    exit 1
  }
  case "$resume_only_rail" in
    fiat|tap|tnk) ;;
    *)
      echo "abort: internal payout resume rail is invalid" >&2
      exit 1
      ;;
  esac
fi

failed=0
pending=0
if [[ -z "$resume_only_rail" || "$resume_only_rail" == "fiat" ]]; then
  if [[ "$FIAT_ENABLED" == "1" ]]; then
    fiat_result=0
    resume_prior_rail_work fiat || fiat_result=$?
    if [[ "$fiat_result" == "0" ]]; then
      settle_fiat || fiat_result=$?
    fi
    if [[ "$fiat_result" == "2" ]]; then
      pending=1
    elif [[ "$fiat_result" != "0" ]]; then
      failed=1
    fi
  else
    mark_rail fiat disabled "MAYHEM_FIAT_SETTLEMENT_ENABLED=0"
  fi
fi
if [[ -z "$resume_only_rail" || "$resume_only_rail" == "tnk" ]]; then
  if [[ "$TNK_ENABLED" == "1" ]]; then
    tnk_result=0
    resume_prior_rail_work tnk || tnk_result=$?
    if [[ "$tnk_result" == "0" ]]; then
      settle_tnk || tnk_result=$?
    fi
    if [[ "$tnk_result" == "2" ]]; then
      pending=1
    elif [[ "$tnk_result" != "0" ]]; then
      failed=1
    fi
  else
    mark_rail tnk disabled "MAYHEM_TNK_SETTLEMENT_ENABLED=0"
  fi
fi
if [[ -z "$resume_only_rail" || "$resume_only_rail" == "tap" ]]; then
  if [[ "$TAP_ENABLED" == "1" ]]; then
    tap_result=0
    resume_prior_rail_work tap || tap_result=$?
    if [[ "$tap_result" == "0" ]]; then
      produce_tap_work || tap_result=$?
    fi
    if [[ "$tap_result" == "2" ]]; then
      pending=1
    elif [[ "$tap_result" != "0" ]]; then
      failed=1
    fi
  else
    mark_rail tap disabled "MAYHEM_TAP_SETTLEMENT_ENABLED=0"
  fi
fi

for rail in fiat tap tnk; do
  [[ -z "$resume_only_rail" || "$resume_only_rail" == "$rail" ]] || continue
  if [[ -f "$work_dir/$rail.complete" ]]; then
    validate_rail_marker "$rail" || failed=1
  fi
done

python3 - "$work_dir/summary.json.tmp" "$work_dir" "$applied_epoch" "$apply_hash" <<'PY'
import json, os, sys
target, work_dir, epoch, apply_hash = sys.argv[1:]
rails = {}
for rail in ("fiat", "tap", "tnk"):
    complete = os.path.join(work_dir, f"{rail}.complete")
    attempts = os.path.join(work_dir, f"{rail}.attempts")
    rails[rail] = {
        "complete": os.path.exists(complete),
        "attempts": int(open(attempts).read().strip()) if os.path.exists(attempts) else 0,
        "result": json.load(open(complete)) if os.path.exists(complete) else None,
    }
with open(target, "w") as out:
    json.dump({
        "schema_version": 1,
        "epoch": int(epoch),
        "epoch_apply_hash": apply_hash,
        "rails": rails,
        "complete": all(item["complete"] for item in rails.values()),
    }, out, indent=2)
    out.write("\n")
PY
mv "$work_dir/summary.json.tmp" "$work_dir/summary.json"

if [[ -n "$resume_only_rail" ]]; then
  if (( failed != 0 )); then
    echo "epoch $applied_epoch $resume_only_rail payout reconciliation remains incomplete (see $work_dir)" >&2
    exit 1
  fi
  if (( pending != 0 )); then
    echo "epoch $applied_epoch $resume_only_rail payout work remains queued"
    exit 0
  fi
  [[ -f "$work_dir/$resume_only_rail.complete" ]] || {
    echo "epoch $applied_epoch $resume_only_rail payout resume produced no completion marker" >&2
    exit 1
  }
  echo "epoch $applied_epoch $resume_only_rail payout work reconciled"
  exit 0
fi

if (( failed != 0 )); then
  echo "epoch $applied_epoch payout reconciliation remains incomplete (see $work_dir)" >&2
  exit 1
fi
if (( pending != 0 )); then
  echo "epoch $applied_epoch payout work is queued; awaiting TAP processed evidence"
  exit 0
fi
python3 - "$work_dir/complete.tmp" "$work_dir/summary.json" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, sys
path, summary_path, epoch, apply_hash = sys.argv[1:]
summary_raw = open(summary_path, "rb").read()
summary = json.loads(summary_raw)
if summary.get("complete") is not True:
    raise SystemExit("refusing to mark an incomplete payout summary complete")
with open(path, "w") as out:
    json.dump({
        "schema_version": 1,
        "type": "epoch_payout_complete",
        "rail": "all",
        "epoch": int(epoch),
        "epoch_apply_hash": apply_hash,
        "summary_sha256": hashlib.sha256(summary_raw).hexdigest(),
    }, out, indent=2)
    out.write("\n")
PY
mv "$work_dir/complete.tmp" "$work_dir/complete"
echo "epoch $applied_epoch payout work reconciled; evidence in $work_dir"
