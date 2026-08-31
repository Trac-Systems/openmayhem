#!/usr/bin/env bash
set -euo pipefail

umask 077

repo="${MAYHEM_REPO:-/opt/mayhem/source}"
spool="${MAYHEM_TAP_SETTLEMENT_SPOOL:-/opt/mayhem/.mayhem-local/settlement/tap}"
interval="${MAYHEM_TAP_SETTLEMENT_INTERVAL_SECONDS:-30}"
attempt_timeout="${MAYHEM_TAP_SETTLEMENT_ATTEMPT_TIMEOUT_SECONDS:-900}"
peer_rpc="${MAYHEM_PEER_RPC:-http://127.0.0.1:49223/v1}"
ready="$spool/ready"
working="$spool/working"
processed="$spool/processed"
failed="$spool/failed"

for value in "$interval" "$attempt_timeout"; do
  [[ "$value" =~ ^[1-9][0-9]*$ ]] && \
    python3 -c 'import sys; raise SystemExit(0 if int(sys.argv[1]) <= 9007199254740991 else 1)' "$value" || {
    echo "TAP settlement interval and attempt timeout must be positive canonical integers" >&2
    exit 1
  }
done
[[ "${MAYHEM_NETWORK:-}" == "mainnet" && "${MAYHEM_TAP_ETH_CHAIN_ID:-}" == "1" ]] || {
  echo "Refusing TAP settlement outside canonical Ethereum mainnet settings" >&2
  exit 1
}
[[ "${MAYHEM_TAP_POOL_ADDRESS:-}" == "0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28" ]] || {
  echo "Refusing TAP settlement for a non-canonical pool" >&2
  exit 1
}
[[ "${MAYHEM_TAP_ROLLER_PRIVATE_KEY:-}" =~ ^0x[0-9a-fA-F]{64}$ && \
   "${MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY:-}" =~ ^0x[0-9a-fA-F]{64}$ && \
   "${MAYHEM_TAP_ROLLER_PRIVATE_KEY:-}" != "${MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY:-}" ]] || {
  echo "Refusing TAP settlement without distinct live owner and governance signers" >&2
  exit 1
}
command -v flock >/dev/null 2>&1 || {
  echo "flock is required to serialize the TAP settlement roller" >&2
  exit 1
}
command -v timeout >/dev/null 2>&1 || {
  echo "timeout is required to bound TAP settlement attempts" >&2
  exit 1
}

mkdir -p "$ready" "$working" "$processed" "$failed"
exec 8>"$spool/roller.lock"
flock -n 8 || {
  echo "Another TAP settlement roller already owns $spool" >&2
  exit 0
}

node "$repo/scripts/mainnet-proof.mjs" \
  --peer-rpc "$peer_rpc" --timeout-seconds 0 --json >"$spool/mainnet-proof.json.tmp"
mv "$spool/mainnet-proof.json.tmp" "$spool/mainnet-proof.json"

while true; do
  bundle="$(find "$working" -maxdepth 1 -type f -name 'epoch-*.receipts.json' -print | sort -V | head -n 1)"
  if [[ -z "$bundle" ]]; then
    candidate="$(find "$ready" -maxdepth 1 -type f -name 'epoch-*.receipts.json' -print | sort -V | head -n 1)"
    if [[ -n "$candidate" ]]; then
      filename="$(basename "$candidate")"
      bundle="$working/$filename"
      mv "$candidate" "$bundle"
      buyer_candidate="$ready/${filename%.receipts.json}.buyers.json"
      [[ ! -f "$buyer_candidate" ]] || mv "$buyer_candidate" "$working/"
    fi
  fi
  if [[ -z "$bundle" ]]; then
    sleep "$interval"
    continue
  fi

  name="$(basename "$bundle" .receipts.json)"
  lifecycle_count=0
  for state in ready working processed failed; do
    [[ -f "$spool/$state/$name.receipts.json" ]] || continue
    lifecycle_count=$((lifecycle_count + 1))
  done
  if (( lifecycle_count != 1 )); then
    echo "TAP spool item $name exists in $lifecycle_count lifecycle states; refusing duplicate work" >&2
    sleep "$interval"
    continue
  fi
  ready_buyer="$ready/$name.buyers.json"
  working_buyer="$working/$name.buyers.json"
  if [[ -f "$ready_buyer" ]]; then
    [[ ! -f "$working_buyer" ]] || {
      echo "TAP buyer-account companion for $name exists in two lifecycle states" >&2
      sleep "$interval"
      continue
    }
    mv "$ready_buyer" "$working_buyer"
  fi
  buyer_accounts="$working/$name.buyers.json"
  rate_lock="$working/$name.tap-rate.json"
  dry_report="$working/$name.dry-run.json"
  raw_report="$working/$name.settlement.raw.json"
  bound_report="$working/$name.settlement.bound.json"
  final_report="$working/$name.settlement.json"
  fee_bps="$(jq -er '.params.fee_bps' "$bundle")" || {
    echo "$bundle is missing params.fee_bps" >&2
    mv "$bundle" "$failed/"
    [[ ! -f "$buyer_accounts" ]] || mv "$buyer_accounts" "$failed/"
    continue
  }

  args=(
    /usr/bin/node "$repo/contracts/scripts/tap-settlement-roller.mjs"
    --bundle "$bundle"
    --tap-rate-lock "$rate_lock"
    --ledger-fee-bps "$fee_bps"
    --peer-rpc "$peer_rpc"
    --eth-rpc "$MAYHEM_TAP_ETH_RPC"
    --pool "$MAYHEM_TAP_POOL_ADDRESS"
    --operator-address "$MAYHEM_TAP_OPERATOR_ADDRESS"
    --json
  )
  [[ -f "$buyer_accounts" ]] && args+=(--buyer-accounts "$buyer_accounts")
  [[ -f "$processed/latest.json" ]] && args+=(--prior "$processed/latest.json")

  if ! timeout "$attempt_timeout" "${args[@]}" >"$dry_report"; then
    echo "TAP settlement dry-run failed for $name" >&2
    sleep "$interval"
    continue
  fi
  if jq -e '
    .posted == false and
    .blocked != true and
    .root == null and
    .entries == [] and
    .providers == [] and
    .refunds == [] and
    .cumulative_spent_wei == "0" and
    .spent_au == "0" and
    (.checkpoint_outputs == []) and
    (.reason == "provider earnings await challenge or holdback maturity" or
      .reason == "provider earnings are below payout minimum")
  ' "$dry_report" >/dev/null; then
    if ! python3 - "$bundle" "$dry_report" "$bound_report" <<'PY'
import hashlib, json, re, sys
bundle_path, report_path, target = sys.argv[1:]
bundle_raw = open(bundle_path, "rb").read()
bundle = json.loads(bundle_raw)
report = json.load(open(report_path))
epoch = bundle.get("epoch")
apply_hash = bundle.get("epoch_apply_hash")
receipts = bundle.get("receipts")

def uint(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"(0|[1-9][0-9]*)", value):
        raise SystemExit(f"{label} is not a canonical unsigned integer")
    return int(value)

if (
    bundle.get("rail") != "tap"
    or not isinstance(epoch, int)
    or epoch < 1
    or not isinstance(apply_hash, str)
    or not re.fullmatch(r"[0-9a-f]{64}", apply_hash)
    or not isinstance(receipts, list)
    or not receipts
):
    raise SystemExit("TAP carry bundle is missing exact rail/epoch/apply binding")
held_count = report.get("held_receipt_count")
threshold_count = report.get("threshold_held_provider_count")
if (
    report.get("posted") is not False
    or report.get("blocked") is True
    or report.get("root") is not None
    or report.get("entries") != []
    or report.get("providers") != []
    or report.get("refunds") != []
    or report.get("checkpoint_outputs") != []
    or report.get("cumulative_spent_wei") != "0"
    or report.get("spent_au") != "0"
    or report.get("receipt_count") != len(receipts)
    or not isinstance(held_count, int)
    or isinstance(held_count, bool)
    or held_count < 0
    or not isinstance(threshold_count, int)
    or isinstance(threshold_count, bool)
    or threshold_count < 0
    or held_count + threshold_count < 1
):
    raise SystemExit("TAP carry report has payable or unclassified work")
held_au = uint(report.get("held_au"), "TAP held_au")
threshold_au = uint(report.get("threshold_held_au"), "TAP threshold_held_au")
payout_min_au = uint(report.get("payout_min_au"), "TAP payout_min_au")
if held_au < threshold_au or (held_count == 0 and held_au != threshold_au):
    raise SystemExit("TAP carry report held totals are inconsistent")
rate_lock = report.get("tap_rate_lock")
minimum_lock = report.get("payout_minimum_lock")
if (
    not isinstance(rate_lock, dict)
    or rate_lock.get("epoch") != epoch
    or not isinstance(rate_lock.get("rate_ts"), int)
    or not isinstance(minimum_lock, dict)
    or minimum_lock.get("type") != "tap_payout_minimum_lock"
    or minimum_lock.get("key") != "params/payout_min_au"
    or minimum_lock.get("at") != rate_lock.get("rate_ts")
    or minimum_lock.get("value") != str(payout_min_au)
    or not isinstance(minimum_lock.get("signed_length"), int)
    or minimum_lock.get("signed_length") < 1
):
    raise SystemExit("TAP carry report lacks confirmed payout policy evidence")
report["status"] = "no_work"
report["outcome"] = "carry"
report["rail"] = "tap"
report["epoch"] = epoch
report["epoch_apply_hash"] = apply_hash
report["bundle_sha256"] = hashlib.sha256(bundle_raw).hexdigest()
report["tap_settlement_checkpoint"] = None
with open(target, "w") as out:
    json.dump(report, out, indent=2)
    out.write("\n")
PY
    then
      echo "TAP carry report binding failed for $name" >&2
      sleep "$interval"
      continue
    fi
    install -m 600 "$bound_report" "$processed/$name.settlement.json"
    install -m 600 "$bound_report" "$processed/latest.json"
    install -m 600 "$rate_lock" "$processed/$name.tap-rate.json"
    mv "$bundle" "$processed/"
    [[ -f "$buyer_accounts" ]] && mv "$buyer_accounts" "$processed/"
    rm -f "$rate_lock" "$dry_report" "$raw_report" "$bound_report" "$final_report"
    continue
  fi
  if ! jq -e '
    .blocked != true and
    (.root | type == "string") and
    (.root_already_posted == true or .propose_root_dry_run.ok == true)
  ' "$dry_report" >/dev/null; then
    echo "TAP settlement dry-run did not pass for $name" >&2
    sleep "$interval"
    continue
  fi
  if ! timeout "$attempt_timeout" "${args[@]}" --confirm >"$raw_report"; then
    echo "TAP settlement broadcast failed for $name" >&2
    sleep "$interval"
    continue
  fi
  if ! python3 - "$bundle" "$raw_report" "$bound_report" <<'PY'
import hashlib, json, re, sys
bundle_path, report_path, target = sys.argv[1:]
bundle_raw = open(bundle_path, "rb").read()
bundle = json.loads(bundle_raw)
report = json.load(open(report_path))
epoch = bundle.get("epoch")
apply_hash = bundle.get("epoch_apply_hash")
if (
    bundle.get("rail") != "tap"
    or not isinstance(epoch, int)
    or epoch < 1
    or not isinstance(apply_hash, str)
    or not re.fullmatch(r"[0-9a-f]{64}", apply_hash)
):
    raise SystemExit("TAP spool bundle is missing exact rail/epoch/apply binding")
if report.get("epoch") != epoch:
    raise SystemExit("TAP settlement report epoch does not match the spool bundle")
root = report.get("root")
if not isinstance(root, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", root):
    raise SystemExit("TAP settlement report root is not a 32-byte hash")
tx = report.get("execution_tx") or report.get("proposal_tx")
if report.get("root_already_posted") is not True and (
    not isinstance(tx, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", tx)
):
    raise SystemExit("TAP settlement report root transaction is invalid")
for field in ("operator_fee", "burn"):
    record = report.get(field)
    if not isinstance(record, dict):
        raise SystemExit(f"TAP settlement report {field} evidence is missing")
    predicted = record.get("predicted_claimable_wei")
    if not isinstance(predicted, str) or not predicted.isdigit():
        raise SystemExit(f"TAP settlement report {field} amount is invalid")
    if predicted != "0" and (
        record.get("auto_sent") is not True
        or not isinstance(record.get("tx"), str)
        or not re.fullmatch(r"0x[0-9a-fA-F]{64}", record["tx"])
    ):
        raise SystemExit(f"TAP settlement report {field} transaction is invalid")
report["rail"] = "tap"
report["epoch_apply_hash"] = apply_hash
report["bundle_sha256"] = hashlib.sha256(bundle_raw).hexdigest()
with open(target, "w") as out:
    json.dump(report, out, indent=2)
    out.write("\n")
PY
  then
    echo "TAP settlement report binding failed for $name" >&2
    sleep "$interval"
    continue
  fi
  mv "$bound_report" "$final_report"
  rm -f "$raw_report"
  if jq -e '
    .blocked != true and
    .root_pending == true and
    .awaiting_governance_delay == true and
    (.execute_after | type == "number")
  ' "$final_report" >/dev/null; then
    echo "TAP settlement root for $name is cross-signed and waiting for its governance delay." >&2
    sleep "$interval"
    continue
  fi
  if ! jq -e '
    .blocked != true and
    .root_confirmed == true and
    (
      .root_already_posted == true or
      (.posted == true and ((.execution_tx | type == "string") or (.proposal_tx | type == "string")))
    ) and
    .operator_fee.completed == true and
    .operator_fee.remaining_claimable_wei == "0" and
    (.operator_fee.predicted_claimable_wei == "0" or .operator_fee.auto_sent == true) and
    .burn.completed == true and
    .burn.remaining_claimable_wei == "0" and
    (.burn.predicted_claimable_wei == "0" or .burn.auto_sent == true)
  ' "$final_report" >/dev/null; then
    echo "TAP settlement report did not confirm root, fee, and burn completion for $name" >&2
    sleep "$interval"
    continue
  fi

  install -m 600 "$final_report" "$processed/$name.settlement.json"
  install -m 600 "$final_report" "$processed/latest.json"
  install -m 600 "$rate_lock" "$processed/$name.tap-rate.json"
  mv "$bundle" "$processed/"
  [[ -f "$buyer_accounts" ]] && mv "$buyer_accounts" "$processed/"
  rm -f "$rate_lock" "$dry_report" "$raw_report" "$bound_report" "$final_report"
done
