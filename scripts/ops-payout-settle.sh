#!/usr/bin/env bash
# Reconcile all operator-owned payout rails for one canonically applied epoch.
# Fiat and TNK execute from ledger liabilities through the mayhem CLI. TAP work
# is derived only from the retained finalized-epoch bundle and handed to the
# existing simulate-first TAP settlement worker.
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
FIAT_OPERATOR_CURRENCY="${MAYHEM_FIAT_OPERATOR_CURRENCY:-eur}"
FIAT_STRIPE_ENV_FILE="${MAYHEM_FIAT_STRIPE_ENV_FILE:-}"
FIAT_TRANSFER_MAX_ATTEMPTS="${MAYHEM_FIAT_TRANSFER_MAX_ATTEMPTS:-4}"
FIAT_TRANSFER_RETRY_MS="${MAYHEM_FIAT_TRANSFER_RETRY_MS:-500}"
TNK_TRANSFER_TIMEOUT_SECONDS="${MAYHEM_TNK_TRANSFER_TIMEOUT_SECONDS:-180}"
TNK_TRANSFER_MAX_RETRIES="${MAYHEM_TNK_TRANSFER_MAX_RETRIES:-3}"

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
  "$TNK_TRANSFER_MAX_RETRIES"; do
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

apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
applied_epoch="$(printf '%s' "$apply_state" | json_field value.updated_epoch)"
pending_epoch="$(printf '%s' "$apply_state" | json_field value.pending_epoch)"
apply_hash="$(printf '%s' "$apply_state" | json_field value.last_apply_hash)"
requested_epoch="${1:-$applied_epoch}"

[[ "$applied_epoch" =~ ^[0-9]+$ ]] || {
  echo "abort: canonical epoch/apply/state.updated_epoch is missing or invalid" >&2
  exit 1
}
if [[ "$applied_epoch" == "0" ]]; then
  echo "skip: no epoch has been finalized yet"
  exit 0
fi
if [[ -n "$pending_epoch" ]]; then
  echo "skip: epoch $pending_epoch still has a pending paged apply"
  exit 0
fi
if [[ "$requested_epoch" != "$applied_epoch" ]]; then
  echo "abort: payout epoch $requested_epoch is not the current applied epoch $applied_epoch" >&2
  exit 1
fi
[[ "$requested_epoch" =~ ^[1-9][0-9]*$ ]] || {
  echo "abort: requested payout epoch must be a positive canonical integer" >&2
  exit 1
}
[[ "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
  echo "abort: canonical epoch/apply/state.last_apply_hash is not a 32-byte hash" >&2
  exit 1
}
apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"

work_dir="$STATE_DIR/payout/epoch-$applied_epoch-$apply_hash"
epoch_dir="$STATE_DIR/epochs/epoch-$applied_epoch"
mkdir -p "$work_dir"
printf '%s\n' "$apply_state" >"$work_dir/apply-state.json.tmp"
mv "$work_dir/apply-state.json.tmp" "$work_dir/apply-state.json"
validate_execution_context

validate_epoch_artifact() {
  local bundle="$epoch_dir/epoch-bundle.json"
  local recomputed="$epoch_dir/epoch-recomputed.json"
  local artifact="$epoch_dir/epoch-artifact.json"
  local gateway_receipts="$epoch_dir/gateway-receipts.json"
  [[ -f "$bundle" ]] || return 0
  [[ -f "$recomputed" && -f "$artifact" && -f "$gateway_receipts" ]] || {
    echo "abort: retained epoch bundle is missing recomputed, gateway, or apply-bound evidence" >&2
    return 1
  }
  local dep_state use_state earn_state fee_state price_state
  dep_state="$(curl -sf -m 10 "$RPC_URL/state?key=ev/dep/$applied_epoch")"
  use_state="$(curl -sf -m 10 "$RPC_URL/state?key=ev/use/$applied_epoch")"
  earn_state="$(curl -sf -m 10 "$RPC_URL/state?key=ev/earn/$applied_epoch")"
  fee_state="$(curl -sf -m 10 "$RPC_URL/state?key=ev/fee/$applied_epoch")"
  price_state="$(curl -sf -m 10 "$RPC_URL/state?key=ev/price/$applied_epoch")"
  python3 - "$bundle" "$recomputed" "$gateway_receipts" "$artifact" \
    "$applied_epoch" "$apply_hash" \
    "$dep_state" "$use_state" "$earn_state" "$fee_state" "$price_state" <<'PY'
import hashlib, json, re, sys
(
    bundle_path,
    recomputed_path,
    receipts_path,
    artifact_path,
    expected_epoch,
    expected_hash,
    dep_raw,
    use_raw,
    earn_raw,
    fee_raw,
    price_raw,
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
if not isinstance(receipts, list) or bundle.get("receipts") != receipts:
    raise SystemExit("retained bundle receipts do not match the exact gateway snapshot")
if artifact.get("schema_version") != 1 or artifact.get("type") != "retained_epoch_artifact":
    raise SystemExit("retained epoch artifact has the wrong schema")
if artifact.get("rail") != "all" or artifact.get("rails") != ["fiat", "tap", "tnk"]:
    raise SystemExit("retained epoch artifact has invalid rail binding")
if artifact.get("epoch") != expected_epoch or artifact.get("epoch_apply_hash") != expected_hash:
    raise SystemExit("retained epoch artifact does not match the canonical epoch/apply hash")
if artifact.get("bundle_sha256") != hashlib.sha256(bundle_raw).hexdigest():
    raise SystemExit("retained epoch bundle hash does not match its artifact")
if artifact.get("recomputed_sha256") != hashlib.sha256(recomputed_raw).hexdigest():
    raise SystemExit("retained recomputed report hash does not match its artifact")
if artifact.get("gateway_receipts_sha256") != hashlib.sha256(receipts_raw).hexdigest():
    raise SystemExit("retained gateway receipt hash does not match its artifact")
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
states = {
    "dep": (json.loads(dep_raw).get("value"), "deposit_root"),
    "use": (json.loads(use_raw).get("value"), "usage_root"),
    "earn": (json.loads(earn_raw).get("value"), "earn_root"),
    "fee": (json.loads(fee_raw).get("value"), "fee_root"),
    "price": (json.loads(price_raw).get("value"), "price_root"),
}
for kind, (state, expected_type) in states.items():
    if (
        not isinstance(state, dict)
        or state.get("type") != expected_type
        or state.get("epoch") != expected_epoch
        or state.get("merkle_root") != roots[kind]
    ):
        raise SystemExit(f"canonical ev/{kind} root does not match retained epoch evidence")
checks = {
    "dep": {"count": totals.get("dep_count"), "au_total": totals.get("dep_au")},
    "use": {
        "sessions": totals.get("use_count"),
        "au_total": totals.get("use_au"),
        "providers": totals.get("provider_count"),
    },
    "earn": {
        "provider_count": totals.get("provider_count"),
        "au_cum_total": totals.get("earn_au"),
    },
    "fee": {
        "au_fee_epoch": totals.get("fee_au"),
        "au_fee_cum": totals.get("fee_cum_au"),
        "au_burn_epoch": totals.get("burn_au"),
        "au_burn_cum": totals.get("burn_cum_au"),
    },
    "price": {"price_count": totals.get("price_count")},
}
for kind, fields in checks.items():
    for field, expected in fields.items():
        if states[kind][0].get(field) != expected:
            raise SystemExit(f"canonical ev/{kind}.{field} does not match retained epoch totals")
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
    "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, os, sys
path, rail, status, evidence, epoch, apply_hash = sys.argv[1:]
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
        "evidence": evidence,
        "evidence_sha256": evidence_sha256,
    }, out, indent=2)
    out.write("\n")
PY
  mv "$work_dir/$rail.complete.tmp" "$work_dir/$rail.complete"
}

validate_rail_marker() {
  local rail="$1"
  python3 - "$work_dir/$rail.complete" "$rail" "$applied_epoch" "$apply_hash" <<'PY'
import hashlib, json, os, sys
path, expected_rail, expected_epoch, expected_hash = sys.argv[1:]
d = json.load(open(path))
if (
    d.get("schema_version") != 1
    or d.get("rail") != expected_rail
    or d.get("epoch") != int(expected_epoch)
    or d.get("epoch_apply_hash") != expected_hash
    or d.get("status") not in {"settled", "already_settled", "no_work", "disabled"}
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
  python3 - "$report" "$rail" "$phase" "$applied_epoch" "$apply_hash" <<'PY'
import json, re, sys
path, rail, phase, expected_epoch, expected_hash = sys.argv[1:]
expected_epoch = int(expected_epoch)
d = json.load(open(path))
if d.get("ok") is not True or d.get("epoch") != expected_epoch:
    raise SystemExit(f"{rail} report outer epoch or ok flag is invalid")
settlement = d.get("settlement")
already = d.get("already_settled")
if not isinstance(settlement, dict):
    raise SystemExit(f"{rail} report is missing settlement payload")
if already is not None and already != settlement:
    raise SystemExit(f"{rail} report outer and retained settlement evidence disagree")
expected_op = f"settle_targeted_{rail}"
if (
    settlement.get("op") != expected_op
    or settlement.get("rail") != rail
    or settlement.get("epoch") != expected_epoch
    or str(settlement.get("epoch_apply_hash", "")).lower() != expected_hash
):
    raise SystemExit(f"{rail} settlement payload is stale or rail-contaminated")
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
elif d.get("nothing_to_settle") is True:
    if outputs or transfers:
        raise SystemExit(f"{rail} no-work report contains transfer work")
elif phase == "final":
    state = d.get("settlement_state")
    if not isinstance(state, dict):
        raise SystemExit(f"{rail} final report has no canonical settlement state")
    for key in ("op", "rail", "epoch", "epoch_apply_hash", "transfer_root", "outputs", transfer_key):
        if state.get(key) != settlement.get(key):
            raise SystemExit(f"{rail} canonical state disagrees on {key}")
    if d.get("submitted") is not True or len(transfers) != len(outputs) or not transfers:
        raise SystemExit(f"{rail} final transfer evidence is incomplete")

if rail == "fiat":
    if phase == "final" and already is None and d.get("nothing_to_settle") is not True:
        reconciliation = d.get("reconciliation") or {}
        if reconciliation.get("all_provider_transfers_verified") is not True:
            raise SystemExit("fiat provider transfer reconciliation is incomplete")
        provider_outputs = [
            (index, output) for index, output in enumerate(outputs)
            if output.get("role") == "provider"
        ]
        outer = d.get("stripe_transfers")
        if not isinstance(outer, list) or len(outer) != len(provider_outputs):
            raise SystemExit("fiat Stripe transfer reports do not cover provider outputs")
        by_index = {item.get("output_index"): item for item in outer}
        for index, _ in provider_outputs:
            report = by_index.get(index)
            transfer = report.get("transfer") if isinstance(report, dict) else None
            evidence = transfers[index]
            if (
                not isinstance(transfer, dict)
                or transfer.get("verified") is not True
                or transfer.get("id") != evidence.get("ref")
            ):
                raise SystemExit("fiat Stripe transfer report disagrees with retained evidence")
else:
    if phase == "final" and already is None:
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

settle_fiat() {
  [[ -f "$work_dir/fiat.complete" ]] && {
    validate_rail_marker fiat || return 1
    echo "fiat: already reconciled for epoch $applied_epoch"
    return 0
  }

  local attempt attempt_result=0 plan_tmp plan_file error_file
  attempt="$(next_attempt fiat)" || attempt_result=$?
  [[ "$attempt_result" == "0" ]] || return "$attempt_result"
  plan_tmp="$work_dir/fiat-plan.json.tmp"
  plan_file="$work_dir/fiat-plan.json"
  error_file="$work_dir/fiat-attempt-$attempt.stderr.log"
  local -a args=(
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --peer-store-name "$ADMIN_STORE"
    --epoch "$applied_epoch"
    --at "$settlement_at"
    --operator-stripe-account "$FIAT_OPERATOR_ACCOUNT"
    --operator-currency "$FIAT_OPERATOR_CURRENCY"
    --stripe-transfer-max-attempts "$FIAT_TRANSFER_MAX_ATTEMPTS"
    --stripe-transfer-retry-ms "$FIAT_TRANSFER_RETRY_MS"
    --json
  )
  [[ -n "$FIAT_STRIPE_ENV_FILE" ]] && args+=(--stripe-env-file "$FIAT_STRIPE_ENV_FILE")

  if ! "$MAYHEM_BIN" admin fiat-settlement "${args[@]}" >"$plan_tmp" 2>"$error_file"; then
    rm -f "$plan_tmp"
    echo "fiat: planning failed on attempt $attempt (see $error_file)" >&2
    return 1
  fi
  mv "$plan_tmp" "$plan_file"
  if ! validate_settlement_report "$plan_file" fiat plan; then
    echo "fiat: plan is not bound to the current rail/epoch/apply hash (see $plan_file)" >&2
    return 1
  fi

  local plan_state fiat_ok fiat_already fiat_empty fiat_blocking
  plan_state="$(python3 - "$plan_file" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
blocking = sum(1 for item in d.get("skipped_providers", []) if item.get("blocking", True))
print(
    str(bool(d.get("ok"))).lower(),
    str(d.get("already_settled") is not None).lower(),
    str(bool(d.get("nothing_to_settle"))).lower(),
    blocking,
)
PY
)"
  read -r fiat_ok fiat_already fiat_empty fiat_blocking <<<"$plan_state"
  if [[ "$fiat_ok" != "true" || "$fiat_blocking" != "0" ]]; then
    echo "fiat: plan has blocking payout errors (see $plan_file)" >&2
    return 1
  fi
  if [[ "$fiat_already" == "true" ]]; then
    mark_rail fiat already_settled "$plan_file"
    echo "fiat: canonical settlement evidence already exists"
    return 0
  fi
  if [[ "$fiat_empty" == "true" ]]; then
    mark_rail fiat no_work "$plan_file"
    echo "fiat: no whole-minor-unit settlement output"
    return 0
  fi

  local final_tmp="$work_dir/fiat-settlement.json.tmp"
  local final_file="$work_dir/fiat-settlement.json"
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
  mark_rail fiat settled "$final_file"
  echo "fiat: Stripe transfers and canonical evidence reconciled"
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
    if grep -Fq \
      "TNK settlement has no positive provider or operator fee outputs; nothing to broadcast" \
      "$error_file"; then
      local liabilities fee_state outstanding
      liabilities="$(curl -sf -m 10 \
        "$RPC_URL/state?prefix=payout/liability/tnk/&confirmed=false&limit=1000")"
      fee_state="$(curl -sf -m 10 "$RPC_URL/state?key=fee/tnk/cum")"
      printf '%s\n' "$liabilities" >"$work_dir/tnk-liabilities.json.tmp"
      mv "$work_dir/tnk-liabilities.json.tmp" "$work_dir/tnk-liabilities.json"
      printf '%s\n' "$fee_state" >"$work_dir/tnk-fee-state.json.tmp"
      mv "$work_dir/tnk-fee-state.json.tmp" "$work_dir/tnk-fee-state.json"
      outstanding="$(python3 - "$liabilities" "$fee_state" <<'PY'
import json, sys
liabilities = json.loads(sys.argv[1]).get("values", [])
fee = json.loads(sys.argv[2]).get("value") or {}
if not isinstance(liabilities, list) or len(liabilities) >= 1000:
    raise SystemExit("TNK liability proof is missing or reached the unpageable RPC limit")
if not isinstance(fee, dict):
    raise SystemExit("TNK fee proof is malformed")

def au(value, label):
    text = str(value)
    if not text.isdigit():
        raise SystemExit(f"{label} is not a canonical decimal amount")
    return int(text)

unpaid = any(
    au(item.get("value", {}).get("total_au", "0"), "TNK liability total_au")
    > au(item.get("value", {}).get("paid_cum_au", "0"), "TNK liability paid_cum_au")
    for item in liabilities
    if str(item.get("key", "")).startswith("payout/liability/tnk/")
)
unpaid_fee = (
    au(fee.get("cum_au", "0"), "TNK fee cum_au")
    > au(fee.get("swept_cum_au", "0"), "TNK fee swept_cum_au")
)
print("true" if unpaid or unpaid_fee else "false")
PY
)"
      if [[ "$outstanding" == "true" ]]; then
        echo "tnk: CLI produced no outputs while canonical TNK liabilities remain held or blocked" >&2
        return 1
      fi
      [[ "$outstanding" == "false" ]] || {
        echo "tnk: liability no-work proof did not return a canonical boolean" >&2
        return 1
      }
      python3 - "$plan_file" "$applied_epoch" "$apply_hash" \
        "$work_dir/tnk-liabilities.json" "$work_dir/tnk-fee-state.json" <<'PY'
import hashlib, json, sys
liabilities_path, fee_path = sys.argv[4:]
with open(sys.argv[1], "w") as out:
    json.dump({
        "schema_version": 1,
        "type": "tnk_no_work_proof",
        "ok": True,
        "nothing_to_settle": True,
        "epoch": int(sys.argv[2]),
        "epoch_apply_hash": sys.argv[3],
        "rail": "tnk",
        "outstanding": False,
        "liabilities_sha256": hashlib.sha256(open(liabilities_path, "rb").read()).hexdigest(),
        "fee_state_sha256": hashlib.sha256(open(fee_path, "rb").read()).hexdigest(),
    }, out, indent=2)
    out.write("\n")
PY
      mark_rail tnk no_work "$plan_file"
      echo "tnk: no positive settlement output"
      return 0
    fi
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
  mark_rail tnk settled "$final_file"
  echo "tnk: MSB transfers and canonical evidence reconciled"
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
          ! validate_tap_processed "$processed_report" "$spool_path"; then
          echo "tap: processed spool item lacks exact settlement evidence at $processed_report" >&2
          return 1
        fi
        mark_rail tap settled "$processed_report"
        echo "tap: root, operator fee, and burn completion reconciled"
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
          ! validate_tap_processed "$processed_report" "$existing"; then
        echo "tap: processed spool item lacks complete settlement evidence at $processed_report" >&2
        return 1
        fi
        mark_rail tap settled "$processed_report"
        echo "tap: canonical processed settlement evidence already exists"
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

failed=0
pending=0
if [[ "$FIAT_ENABLED" == "1" ]]; then
  fiat_result=0
  settle_fiat || fiat_result=$?
  if [[ "$fiat_result" == "2" ]]; then
    pending=1
  elif [[ "$fiat_result" != "0" ]]; then
    failed=1
  fi
else
  mark_rail fiat disabled "MAYHEM_FIAT_SETTLEMENT_ENABLED=0"
fi
if [[ "$TNK_ENABLED" == "1" ]]; then
  tnk_result=0
  settle_tnk || tnk_result=$?
  if [[ "$tnk_result" == "2" ]]; then
    pending=1
  elif [[ "$tnk_result" != "0" ]]; then
    failed=1
  fi
else
  mark_rail tnk disabled "MAYHEM_TNK_SETTLEMENT_ENABLED=0"
fi
if [[ "$TAP_ENABLED" == "1" ]]; then
  tap_result=0
  produce_tap_work || tap_result=$?
  if [[ "$tap_result" == "2" ]]; then
    pending=1
  elif [[ "$tap_result" != "0" ]]; then
    failed=1
  fi
else
  mark_rail tap disabled "MAYHEM_TAP_SETTLEMENT_ENABLED=0"
fi

for rail in fiat tap tnk; do
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
