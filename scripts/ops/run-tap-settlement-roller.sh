#!/usr/bin/env bash
set -euo pipefail

repo="${MAYHEM_REPO:-/opt/mayhem/source}"
spool="${MAYHEM_TAP_SETTLEMENT_SPOOL:-/opt/mayhem/.mayhem-local/settlement/tap}"
interval="${MAYHEM_TAP_SETTLEMENT_INTERVAL_SECONDS:-30}"
ready="$spool/ready"
working="$spool/working"
processed="$spool/processed"
failed="$spool/failed"

mkdir -p "$ready" "$working" "$processed" "$failed"

while true; do
  bundle="$(find "$ready" -maxdepth 1 -type f -name 'epoch-*.receipts.json' -print | sort -V | head -n 1)"
  if [[ -z "$bundle" ]]; then
    sleep "$interval"
    continue
  fi

  name="$(basename "$bundle" .receipts.json)"
  buyer_accounts="$ready/$name.buyers.json"
  dry_report="$working/$name.dry-run.json"
  final_report="$working/$name.settlement.json"
  fee_bps="$(jq -er '.params.fee_bps' "$bundle")" || {
    echo "$bundle is missing params.fee_bps" >&2
    mv "$bundle" "$failed/"
    continue
  }

  args=(
    /usr/bin/node "$repo/contracts/scripts/tap-settlement-roller.mjs"
    --bundle "$bundle"
    --ledger-fee-bps "$fee_bps"
    --peer-rpc "${MAYHEM_PEER_RPC:-http://127.0.0.1:49223/v1}"
    --eth-rpc "$MAYHEM_TAP_ETH_RPC"
    --pool "$MAYHEM_TAP_POOL_ADDRESS"
    --operator-address "$MAYHEM_TAP_OPERATOR_ADDRESS"
    --json
  )
  [[ -f "$buyer_accounts" ]] && args+=(--buyer-accounts "$buyer_accounts")
  [[ -f "$processed/latest.json" ]] && args+=(--prior "$processed/latest.json")

  if ! "${args[@]}" > "$dry_report"; then
    echo "TAP settlement dry-run failed for $name" >&2
    sleep "$interval"
    continue
  fi
  if ! jq -e '.blocked != true and (.root | type == "string") and (.set_root_dry_run.ok == true)' "$dry_report" >/dev/null; then
    echo "TAP settlement dry-run did not pass for $name" >&2
    sleep "$interval"
    continue
  fi
  if ! "${args[@]}" --confirm > "$final_report"; then
    echo "TAP settlement broadcast failed for $name" >&2
    sleep "$interval"
    continue
  fi
  if ! jq -e '.posted == true and (.tx | type == "string")' "$final_report" >/dev/null; then
    echo "TAP settlement report is not a confirmed broadcast for $name" >&2
    sleep "$interval"
    continue
  fi

  install -m 600 "$final_report" "$processed/$name.settlement.json"
  install -m 600 "$final_report" "$processed/latest.json"
  mv "$bundle" "$processed/"
  [[ -f "$buyer_accounts" ]] && mv "$buyer_accounts" "$processed/"
  rm -f "$dry_report" "$final_report"
done
