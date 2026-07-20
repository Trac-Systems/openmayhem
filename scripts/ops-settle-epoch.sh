#!/usr/bin/env bash
# OpenMayhem retained-receipt epoch finalization (admin server).
# Settles the active billing epoch WITH its retained gateway receipts:
#   receipts export -> recompute -> epoch-commit -> epoch-apply -> payout worker
# Finalization artifacts use a deterministic epoch directory so a retry after
# apply resumes payout reconciliation instead of attempting the next epoch.
# Every epoch submit is preceded by a sim. Aborts on any mismatch.
# Usage: ops-settle-epoch.sh [epoch]   (defaults to updated_epoch + 1)
set -euo pipefail

umask 077

RPC_URL="${MAYHEM_RPC_URL:-http://127.0.0.1:49223/v1}"
GATEWAY_URL="${MAYHEM_GATEWAY_URL:-http://127.0.0.1:11445}"
MAYHEM_BIN="${MAYHEM_BIN:-/opt/mayhem/source/target/release/mayhem}"
ADMIN_HOME="${MAYHEM_ADMIN_HOME:-/opt/mayhem/.mayhem-local/live-home}"
ADMIN_STORE="${MAYHEM_ADMIN_STORE:-mayhem-canonical-admin}"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-/opt/mayhem/source}"
STATE_DIR="${MAYHEM_CADENCE_STATE_DIR:-/opt/mayhem/.mayhem-local/settlement}"

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

validate_execution_context() {
    if [[ "${MAYHEM_PAYOUT_TEST_MODE:-0}" == "1" ]]; then
        local test_root="${MAYHEM_PAYOUT_TEST_ROOT:-}"
        [[ -n "$test_root" && -d "$test_root" ]] || {
            echo "abort: finalizer test mode requires an existing MAYHEM_PAYOUT_TEST_ROOT" >&2
            return 1
        }
        for path in "$MAYHEM_BIN" "$ADMIN_HOME" "$STATE_DIR"; do
            [[ "$path" == "$test_root"/* ]] || {
                echo "abort: finalizer test mode path escapes MAYHEM_PAYOUT_TEST_ROOT: $path" >&2
                return 1
            }
        done
        [[ "$RPC_URL" == "http://mock.invalid/v1" && "$GATEWAY_URL" == "http://gateway.mock.invalid" ]] || {
            echo "abort: finalizer test mode requires isolated mock endpoints" >&2
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
                echo "abort: finalizer test mode refuses inherited credential $key" >&2
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
    [[ "${MAYHEM_TAP_ETH_CHAIN_ID:-}" == "1" && "${MAYHEM_STRIPE_MODE:-}" == "live" ]] || {
        echo "abort: payment rails are not configured for canonical live networks" >&2
        return 1
    }
    [[ "${MAYHEM_STRIPE_API_BASE_URL:-}" == "https://api.stripe.com" && \
       "${MAYHEM_STRIPE_SECRET_KEY:-}" == sk_live_* ]] || {
        echo "abort: Stripe is not configured for the canonical live endpoint" >&2
        return 1
    }
    [[ "${MAYHEM_PAYGATE_CONTRACT_DRY_RUN:-}" == "0" ]] || {
        echo "abort: MAYHEM_PAYGATE_CONTRACT_DRY_RUN must be 0 for live finalization" >&2
        return 1
    }
    [[ "${MAYHEM_PAYGATE_STRIPE_ENABLED:-}" == "1" ]] || {
        echo "abort: MAYHEM_PAYGATE_STRIPE_ENABLED must be 1 for live finalization" >&2
        return 1
    }
    [[ "${MAYHEM_TAP_POOL_ADDRESS:-}" == "0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28" ]] || {
        echo "abort: MAYHEM_TAP_POOL_ADDRESS is not the canonical mainnet pool" >&2
        return 1
    }
    [[ "${MAYHEM_TAP_TOKEN_ADDR:-}" == "0x5e7F6e008C6d9D7AD4c7EB75Bd4ce62864cc7454" && \
       "${MAYHEM_TAP_OPERATOR_ADDRESS:-}" =~ ^0x[0-9a-fA-F]{40}$ ]] || {
        echo "abort: live TAP token or operator setting is invalid" >&2
        return 1
    }
    [[ "${MAYHEM_TAP_ROLLER_PRIVATE_KEY:-}" =~ ^0x[0-9a-fA-F]{64}$ && \
       "${MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY:-}" =~ ^0x[0-9a-fA-F]{64}$ && \
       "${MAYHEM_TAP_ROLLER_PRIVATE_KEY:-}" != "${MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY:-}" ]] || {
        echo "abort: live TAP owner and governance signers are missing, invalid, or identical" >&2
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
    [[ "${MAYHEM_FIAT_SETTLEMENT_ENABLED:-}" == "1" && \
       "${MAYHEM_TAP_SETTLEMENT_ENABLED:-}" == "1" && \
       "${MAYHEM_TNK_SETTLEMENT_ENABLED:-}" == "1" ]] || {
        echo "abort: all automatic payout rails must be enabled before finalization" >&2
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
    if ! node "$SOURCE_DIR/scripts/mainnet-proof.mjs" \
        --peer-rpc "$RPC_URL" --timeout-seconds 0 --json \
        >"$STATE_DIR/finalizer-mainnet-proof.json.tmp"; then
        rm -f "$STATE_DIR/finalizer-mainnet-proof.json.tmp"
        echo "abort: canonical mainnet proof failed; refusing epoch finalization" >&2
        return 1
    fi
    mv "$STATE_DIR/finalizer-mainnet-proof.json.tmp" "$STATE_DIR/finalizer-mainnet-proof.json"
}

bind_epoch_artifact() {
    local epoch="$1" apply_hash="$2" run_dir="$3"
    python3 - "$run_dir/epoch-artifact.json.tmp" "$run_dir/epoch-artifact.json" \
        "$run_dir/epoch-bundle.json" "$run_dir/epoch-recomputed.json" \
        "$run_dir/gateway-receipts.json" \
        "$epoch" "$apply_hash" <<'PY'
import hashlib, json, os, re, sys
target, existing, bundle_path, recomputed_path, receipts_path, epoch, apply_hash = sys.argv[1:]
epoch = int(epoch)

def load(path):
    with open(path, "rb") as source:
        raw = source.read()
    return raw, json.loads(raw)

bundle_raw, bundle = load(bundle_path)
recomputed_raw, recomputed = load(recomputed_path)
receipts_raw, receipts = load(receipts_path)
if bundle.get("epoch") != epoch or recomputed.get("epoch") != epoch:
    raise SystemExit("retained epoch bundle or recomputation has the wrong epoch")
if not isinstance(receipts, list) or bundle.get("receipts") != receipts:
    raise SystemExit("retained bundle does not match the frozen gateway receipt snapshot")
roots = recomputed.get("roots")
if not isinstance(roots, dict) or any(
    not re.fullmatch(r"[0-9a-f]{64}", str(roots.get(key, "")))
    for key in ("dep", "use", "earn", "fee", "price")
):
    raise SystemExit("retained epoch recomputation has invalid roots")
artifact = {
    "schema_version": 1,
    "type": "retained_epoch_artifact",
    "rail": "all",
    "rails": ["fiat", "tap", "tnk"],
    "epoch": epoch,
    "epoch_apply_hash": apply_hash,
    "bundle_sha256": hashlib.sha256(bundle_raw).hexdigest(),
    "recomputed_sha256": hashlib.sha256(recomputed_raw).hexdigest(),
    "gateway_receipts_sha256": hashlib.sha256(receipts_raw).hexdigest(),
    "roots": roots,
    "totals": recomputed.get("totals"),
}
if os.path.exists(existing) and json.load(open(existing)) != artifact:
    raise SystemExit("existing retained epoch artifact is stale or mismatched")
with open(target, "w") as out:
    json.dump(artifact, out, indent=2)
    out.write("\n")
PY
    mv "$run_dir/epoch-artifact.json.tmp" "$run_dir/epoch-artifact.json"
}

mkdir -p "$STATE_DIR/epochs"
if [[ "${MAYHEM_PAYOUT_LOCK_HELD:-0}" != "1" ]]; then
    command -v flock >/dev/null 2>&1 || {
        echo "abort: flock is required for epoch finalization serialization" >&2
        exit 1
    }
    exec 9>"$STATE_DIR/payout.lock"
    if ! flock -n 9; then
        echo "skip: another payout/finalization worker holds $STATE_DIR/payout.lock"
        exit 0
    fi
    export MAYHEM_PAYOUT_LOCK_HELD=1
fi
validate_execution_context

apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
updated_epoch="$(printf '%s' "$apply_state" | json_field value.updated_epoch)"
pending_epoch="$(printf '%s' "$apply_state" | json_field value.pending_epoch)"
non_negative_integer "$updated_epoch" || {
    echo "abort: canonical epoch/apply/state.updated_epoch is missing or invalid" >&2
    exit 1
}
next_epoch=$((updated_epoch + 1))
epoch="${1:-$next_epoch}"
positive_integer "$epoch" || {
    echo "abort: epoch must be a positive canonical integer" >&2
    exit 1
}
if [[ -n "$pending_epoch" ]]; then
    echo "abort: epoch $pending_epoch still has a pending paged apply" >&2
    exit 1
fi
if [[ "$epoch" != "$next_epoch" && "$epoch" != "$updated_epoch" ]]; then
    echo "abort: epoch $epoch is neither next nor current (updated_epoch=$updated_epoch)" >&2
    exit 1
fi

if [[ "$epoch" == "$next_epoch" && "$updated_epoch" != "0" ]]; then
    current_apply_hash="$(printf '%s' "$apply_state" | json_field value.last_apply_hash)"
    [[ "$current_apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
        echo "abort: current applied epoch has no canonical apply hash" >&2
        exit 1
    }
    current_apply_hash="$(printf '%s' "$current_apply_hash" | tr '[:upper:]' '[:lower:]')"
    if ! "$SOURCE_DIR/scripts/ops-payout-settle.sh" "$updated_epoch"; then
        echo "abort: payout reconciliation failed for current epoch $updated_epoch" >&2
        exit 1
    fi
    current_payout_dir="$STATE_DIR/payout/epoch-$updated_epoch-$current_apply_hash"
    [[ -f "$current_payout_dir/complete" ]] || {
        echo "abort: current epoch $updated_epoch payouts remain incomplete; refusing to finalize epoch $epoch" >&2
        exit 1
    }
fi

run_dir="$STATE_DIR/epochs/epoch-$epoch"
mkdir -p "$run_dir"
echo "settling epoch $epoch; artifacts in $run_dir"

if [[ "$epoch" == "$updated_epoch" ]]; then
    apply_hash="$(printf '%s' "$apply_state" | json_field value.last_apply_hash)"
    [[ "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
        echo "abort: applied epoch $epoch has no canonical apply hash" >&2
        exit 1
    }
    apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"
    if [[ -f "$run_dir/epoch-apply-hash" && \
          "$(tr '[:upper:]' '[:lower:]' <"$run_dir/epoch-apply-hash")" != "$apply_hash" ]]; then
        echo "abort: retained epoch artifacts do not match canonical apply hash" >&2
        exit 1
    fi
    for artifact in epoch-bundle.json epoch-recomputed.json gateway-receipts.json; do
        [[ -f "$run_dir/$artifact" ]] || {
            echo "abort: applied epoch retry is missing $run_dir/$artifact" >&2
            exit 1
        }
    done
    printf '%s\n' "$apply_hash" >"$run_dir/epoch-apply-hash.tmp"
    mv "$run_dir/epoch-apply-hash.tmp" "$run_dir/epoch-apply-hash"
    bind_epoch_artifact "$epoch" "$apply_hash" "$run_dir"
    "$SOURCE_DIR/scripts/ops-payout-settle.sh" "$epoch"
    echo "epoch $epoch was already finalized; payout reconciliation resumed"
    exit 0
fi

[[ ! -f "$run_dir/epoch-artifact.json" && ! -f "$run_dir/epoch-apply-hash" ]] || {
    echo "abort: unapplied epoch directory contains stale apply-bound evidence" >&2
    exit 1
}

if [[ ! -f "$run_dir/epoch-bundle.json" ]]; then
    fee_bps="$(curl -sf -m 10 "$RPC_URL/state?key=rules/current" | json_field value.fee_bps)"
    fee_bps="${fee_bps:-1500}"
    [[ "$fee_bps" =~ ^(0|[1-9][0-9]*)$ ]] || {
        echo "abort: canonical fee_bps is not a non-negative integer" >&2
        exit 1
    }
    echo "fee_bps=$fee_bps"

    receipts_response="$run_dir/gateway-receipts-response.json.tmp"
    curl -sf -m 10 "$GATEWAY_URL/mayhem/receipts" >"$receipts_response"
    python3 - "$receipts_response" "$run_dir/gateway-receipts.json.tmp" <<'PY'
import json, sys
response = json.load(open(sys.argv[1]))
receipts = response if isinstance(response, list) else response.get("data")
if not isinstance(receipts, list) or not receipts:
    raise SystemExit("canonical retained-receipt finalizer requires a non-empty gateway receipt list")
with open(sys.argv[2], "w") as out:
    json.dump(receipts, out, indent=2)
    out.write("\n")
PY
    mv "$run_dir/gateway-receipts.json.tmp" "$run_dir/gateway-receipts.json"
    rm -f "$receipts_response"

    # The contract validates totals.earn_au against the providers' NEW
    # CUMULATIVE earnings and fee/burn cums against ledger state, so the bundle
    # must carry prior values for every provider/rail touched by the receipts.
    prior_summary="$(python3 - "$run_dir/gateway-receipts.json" "$RPC_URL" "$run_dir/prior-earnings.json" <<'PY'
import json, sys, urllib.request

receipts_path, rpc, out_path = sys.argv[1], sys.argv[2], sys.argv[3]

def get(url):
    with urllib.request.urlopen(url, timeout=10) as f:
        return json.load(f)

def au(value, label):
    text = str(value)
    if not text or (text != "0" and (not text.isdigit() or text.startswith("0"))):
        raise SystemExit(f"{label} must be a canonical non-negative decimal")
    return int(text)

receipts = json.load(open(receipts_path))
if not isinstance(receipts, list):
    raise SystemExit("gateway receipts response is not a list")
pairs = set()
for stored in receipts:
    if not isinstance(stored, dict):
        raise SystemExit("stored receipt must be an object")
    receipt = stored.get("receipt")
    body = receipt.get("body") if isinstance(receipt, dict) else None
    if not isinstance(body, dict):
        raise SystemExit("stored receipt is missing its signed body")
    outer_rail = stored.get("rail")
    rail = body.get("rail")
    if not isinstance(outer_rail, str) or not isinstance(rail, str):
        raise SystemExit("receipt outer rail and signed body rail are required")
    if outer_rail != outer_rail.lower() or rail != rail.lower() or outer_rail != rail:
        raise SystemExit("receipt outer rail does not match signed body rail")
    if rail not in {"fiat", "tap", "tnk"}:
        raise SystemExit("signed receipt rail is unsupported")
    provider = body.get("provider")
    if not isinstance(provider, str) or not provider:
        raise SystemExit("signed receipt provider is required")
    pairs.add((rail, provider))

prior_earnings = {}
for rail, provider in sorted(pairs):
    record = get(f"{rpc}/state?key=earn/{rail}/{provider}").get("value")
    prior_earnings[f"{rail}/{provider}"] = str(
        au((record or {}).get("total_au", "0"), f"earn/{rail}/{provider}.total_au")
    )

fee_cum = 0
burn_cum = 0

for rail in ("fiat", "tap", "tnk"):
    fee = get(f"{rpc}/state?key=fee/{rail}/cum").get("value")
    burn = get(f"{rpc}/state?key=burn/{rail}/cum").get("value")
    fee_cum += au((fee or {}).get("cum_au", "0"), f"fee/{rail}/cum.cum_au")
    burn_cum += au((burn or {}).get("cum_au", "0"), f"burn/{rail}/cum.cum_au")

json.dump(prior_earnings, open(out_path, "w"), indent=1)
print(f"{fee_cum} {burn_cum}")
PY
    )"
    prior_fee_cum_au="${prior_summary%% *}"
    prior_burn_cum_au="${prior_summary##* }"
    echo "prior_fee_cum_au=$prior_fee_cum_au prior_burn_cum_au=$prior_burn_cum_au"

    bundle_tmp="$run_dir/epoch-bundle.json.tmp"
    export_tmp="$run_dir/export-report.json.tmp"
    "$MAYHEM_BIN" receipts export \
        --epoch "$epoch" \
        --fee-bps "$fee_bps" \
        --home "$ADMIN_HOME" \
        --rpc-url "$RPC_URL" \
        --receipts-file "$run_dir/gateway-receipts.json" \
        --prior-earnings-file "$run_dir/prior-earnings.json" \
        --prior-fee-cum-au "$prior_fee_cum_au" \
        --prior-burn-cum-au "$prior_burn_cum_au" \
        --output "$bundle_tmp" \
        --no-verify \
        --json >"$export_tmp"

    # Fold in the deposit root evidence when this epoch saw deposits.
    dep_root="$(curl -sf -m 10 "$RPC_URL/state?key=ev/dep/$epoch" | json_field value.merkle_root)"
    if [[ -n "$dep_root" ]]; then
        python3 - "$bundle_tmp" "$RPC_URL" "$epoch" <<'PY'
import json, re, sys, urllib.request
bundle_path, rpc, epoch = sys.argv[1], sys.argv[2], sys.argv[3]
with urllib.request.urlopen(f"{rpc}/state?key=ev/dep/{epoch}", timeout=10) as f:
    value = json.load(f)["value"]
if (
    not isinstance(value, dict)
    or not re.fullmatch(r"[0-9a-f]{64}", str(value.get("merkle_root", "")))
    or not isinstance(value.get("count"), int)
    or value["count"] < 0
    or not str(value.get("au_total", "")).isdigit()
):
    raise SystemExit("canonical deposit root evidence is malformed")
bundle = json.load(open(bundle_path))
bundle["deposit_root"] = {
    "merkle_root": value["merkle_root"],
    "count": value["count"],
    "au_total": str(value["au_total"]),
    "source": f"ev/dep/{epoch}",
}
json.dump(bundle, open(bundle_path, "w"), indent=2)
print(f"deposit root {value['merkle_root'][:16]} folded in")
PY
    fi
    mv "$bundle_tmp" "$run_dir/epoch-bundle.json"
    mv "$export_tmp" "$run_dir/export-report.json"
else
    bundle_epoch="$(json_field epoch <"$run_dir/epoch-bundle.json")"
    [[ "$bundle_epoch" == "$epoch" ]] || {
        echo "abort: frozen bundle epoch $bundle_epoch does not match $epoch" >&2
        exit 1
    }
    echo "reusing frozen finalized-receipt bundle for epoch $epoch"
fi

[[ -f "$run_dir/gateway-receipts.json" ]] || {
    echo "abort: frozen epoch bundle is missing its exact gateway receipt snapshot" >&2
    exit 1
}
python3 - "$run_dir/epoch-bundle.json" "$run_dir/gateway-receipts.json" "$epoch" <<'PY'
import json, sys
bundle = json.load(open(sys.argv[1]))
gateway_receipts = json.load(open(sys.argv[2]))
expected_epoch = int(sys.argv[3])
if bundle.get("epoch") != expected_epoch:
    raise SystemExit("frozen receipt bundle epoch is stale")
receipts = bundle.get("receipts")
if not isinstance(receipts, list):
    raise SystemExit("frozen receipt bundle receipts must be a list")
if receipts != gateway_receipts:
    raise SystemExit("frozen receipt bundle does not match its exact gateway snapshot")
for entry in receipts:
    if not isinstance(entry, dict):
        raise SystemExit("frozen receipt entry must be an object")
    receipt = entry.get("receipt")
    body = receipt.get("body") if isinstance(receipt, dict) else None
    if not isinstance(body, dict):
        raise SystemExit("frozen receipt is missing its signed body")
    outer = entry.get("rail")
    inner = body.get("rail")
    if (
        not isinstance(outer, str)
        or not isinstance(inner, str)
        or outer != outer.lower()
        or inner != inner.lower()
        or outer != inner
        or inner not in {"fiat", "tap", "tnk"}
    ):
        raise SystemExit("frozen receipt outer rail does not match signed body rail")
PY

node "$SOURCE_DIR/intercom/scripts/recompute-epoch-roots.mjs" "$run_dir/epoch-bundle.json" \
    >"$run_dir/epoch-recomputed.json.tmp"
mv "$run_dir/epoch-recomputed.json.tmp" "$run_dir/epoch-recomputed.json"
echo "recomputed: use_au=$(json_field totals.use_au <"$run_dir/epoch-recomputed.json") earn_au=$(json_field totals.earn_au <"$run_dir/epoch-recomputed.json")"

if [[ ! -f "$run_dir/finalization-at" ]]; then
    date +%s >"$run_dir/finalization-at.tmp"
    mv "$run_dir/finalization-at.tmp" "$run_dir/finalization-at"
fi
at="$(cat "$run_dir/finalization-at")"
[[ "$at" =~ ^[1-9][0-9]*$ ]] || {
    echo "abort: invalid frozen finalization timestamp" >&2
    exit 1
}
admin_common=(
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --peer-store-name "$ADMIN_STORE"
    --recomputed-file "$run_dir/epoch-recomputed.json"
    --at "$at"
    --submit --json
)

for step in epoch-commit epoch-apply; do
    "$MAYHEM_BIN" admin "$step" "${admin_common[@]}" --sim >"$run_dir/$step-sim.json"
    # epoch-commit reports ok under tx.result.ok (/tx path); epoch-apply
    # reports a top-level ok (feature path).
    sim_ok="$(python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
ok = d.get("ok")
if ok is None:
    ok = d.get("tx", {}).get("result", {}).get("ok")
print(ok)
' "$run_dir/$step-sim.json")"
    if [[ "$sim_ok" != "True" && "$sim_ok" != "true" ]]; then
        echo "abort: $step sim did not return ok (see $run_dir/$step-sim.json)" >&2
        exit 1
    fi
    "$MAYHEM_BIN" admin "$step" "${admin_common[@]}" >"$run_dir/$step.json"
    echo "$step submitted"
done

sleep 3
after_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
after="$(printf '%s' "$after_state" | json_field value.updated_epoch)"
if [[ "$after" != "$epoch" ]]; then
    echo "abort: applied epoch $epoch but updated_epoch is $after" >&2
    exit 1
fi
apply_hash="$(printf '%s' "$after_state" | json_field value.last_apply_hash)"
[[ "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
    echo "abort: applied epoch $epoch did not expose a canonical apply hash" >&2
    exit 1
}
apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"
printf '%s\n' "$apply_hash" >"$run_dir/epoch-apply-hash.tmp"
mv "$run_dir/epoch-apply-hash.tmp" "$run_dir/epoch-apply-hash"
bind_epoch_artifact "$epoch" "$apply_hash" "$run_dir"

date +%s >"$STATE_DIR/cadence.last-advance" 2>/dev/null || true
"$SOURCE_DIR/scripts/ops-payout-settle.sh" "$epoch"
echo "epoch $epoch settled; billing epoch is now $((epoch + 1))"
