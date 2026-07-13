#!/usr/bin/env bash
# OpenMayhem manual epoch settlement (admin server).
# Settles the active billing epoch WITH its retained gateway receipts:
#   receipts export (from gateway) -> recompute-epoch-roots -> epoch-commit -> epoch-apply
# Every submit is preceded by a sim. Aborts on any mismatch.
# Usage: ops-settle-epoch.sh [epoch]   (defaults to updated_epoch + 1)
set -euo pipefail

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

apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
updated_epoch="$(printf '%s' "$apply_state" | json_field value.updated_epoch)"
epoch="${1:-$((updated_epoch + 1))}"
if [[ "$epoch" != "$((updated_epoch + 1))" ]]; then
    echo "abort: epoch $epoch is not contiguous (updated_epoch=$updated_epoch)" >&2
    exit 1
fi

run_dir="$STATE_DIR/manual-epoch-$epoch-$(date +%s)"
mkdir -p "$run_dir"
echo "settling epoch $epoch; artifacts in $run_dir"

fee_bps="$(curl -sf -m 10 "$RPC_URL/state?key=rules/current" | json_field value.fee_bps)"
fee_bps="${fee_bps:-1500}"
echo "fee_bps=$fee_bps"

# The contract validates totals.earn_au against the providers' NEW CUMULATIVE
# earnings and fee/burn cums against ledger state, so the bundle must carry the
# prior ledger values for every provider/rail touched by the receipts.
prior_summary="$(python3 - "$GATEWAY_URL" "$RPC_URL" "$run_dir/prior-earnings.json" <<'PY'
import json, sys, urllib.request

gateway, rpc, out_path = sys.argv[1], sys.argv[2], sys.argv[3]

def get(url):
    with urllib.request.urlopen(url, timeout=10) as f:
        return json.load(f)

receipts = get(f"{gateway}/mayhem/receipts").get("data", [])
pairs = set()
rails = set()
for stored in receipts:
    receipt = stored.get("receipt", {})
    rail = stored.get("rail") or receipt.get("rail")
    provider = receipt.get("provider")
    if rail and provider:
        pairs.add((rail, provider))
        rails.add(rail)

prior_earnings = {}
for rail, provider in sorted(pairs):
    record = get(f"{rpc}/state?key=earn/{rail}/{provider}").get("value")
    prior_earnings[f"{rail}/{provider}"] = str((record or {}).get("total_au", "0"))

fee_cum = 0
burn_cum = 0
for rail in ("fiat", "tap", "tnk"):
    fee = get(f"{rpc}/state?key=fee/{rail}/cum").get("value")
    burn = get(f"{rpc}/state?key=burn/{rail}/cum").get("value")
    fee_cum += int((fee or {}).get("cum_au", "0"))
    burn_cum += int((burn or {}).get("cum_au", "0"))

json.dump(prior_earnings, open(out_path, "w"), indent=1)
print(f"{fee_cum} {burn_cum}")
PY
)"
prior_fee_cum_au="${prior_summary%% *}"
prior_burn_cum_au="${prior_summary##* }"
echo "prior_fee_cum_au=$prior_fee_cum_au prior_burn_cum_au=$prior_burn_cum_au"

"$MAYHEM_BIN" receipts export \
    --epoch "$epoch" \
    --fee-bps "$fee_bps" \
    --gateway-url "$GATEWAY_URL" \
    --prior-earnings-file "$run_dir/prior-earnings.json" \
    --prior-fee-cum-au "$prior_fee_cum_au" \
    --prior-burn-cum-au "$prior_burn_cum_au" \
    --output "$run_dir/epoch-bundle.json" \
    --no-verify \
    --json >"$run_dir/export-report.json"

# Fold in the deposit root evidence when this epoch saw deposits.
dep_root="$(curl -sf -m 10 "$RPC_URL/state?key=ev/dep/$epoch" | json_field value.merkle_root)"
if [[ -n "$dep_root" ]]; then
    python3 - "$run_dir/epoch-bundle.json" "$RPC_URL" "$epoch" <<'PY'
import json, sys, urllib.request
bundle_path, rpc, epoch = sys.argv[1], sys.argv[2], sys.argv[3]
with urllib.request.urlopen(f"{rpc}/state?key=ev/dep/{epoch}", timeout=10) as f:
    value = json.load(f)["value"]
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

node "$SOURCE_DIR/intercom/scripts/recompute-epoch-roots.mjs" "$run_dir/epoch-bundle.json" \
    >"$run_dir/epoch-recomputed.json"
echo "recomputed: use_au=$(json_field totals.use_au <"$run_dir/epoch-recomputed.json") earn_au=$(json_field totals.earn_au <"$run_dir/epoch-recomputed.json")"

at="$(date +%s)"
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
after="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state" | json_field value.updated_epoch)"
if [[ "$after" != "$epoch" ]]; then
    echo "abort: applied epoch $epoch but updated_epoch is $after" >&2
    exit 1
fi
date +%s >"$STATE_DIR/cadence.last-advance" 2>/dev/null || true
echo "epoch $epoch settled; billing epoch is now $((epoch + 1))"
echo "NOTE: settled receipts remain in gateway memory and will block the"
echo "cadence sealer until the gateway restarts or new-epoch receipts settle."
