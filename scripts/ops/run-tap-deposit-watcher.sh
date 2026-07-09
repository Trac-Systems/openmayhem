#!/usr/bin/env bash
set -euo pipefail

repo="${MAYHEM_REPO:-/opt/mayhem/source}"
home="${MAYHEM_HOME:-/opt/mayhem/.mayhem-local/live-home}"
peer_rpc="${MAYHEM_PEER_RPC:-http://127.0.0.1:49223/v1}"
cursor="${MAYHEM_TAP_DEPOSIT_CURSOR:-/opt/mayhem/.mayhem-local/watchers/tap-deposit.json}"
interval="${MAYHEM_TAP_DEPOSIT_INTERVAL_SECONDS:-30}"
mayhem_bin="$repo/target/release/mayhem"

mkdir -p "$(dirname "$cursor")"

while true; do
  pool="${MAYHEM_TAP_POOL_ADDRESS:-}"
  if [[ -z "$pool" || "$pool" == "0x0000000000000000000000000000000000000000" ]]; then
    echo "TAP deposit watcher waiting for MAYHEM_TAP_POOL_ADDRESS"
    sleep "$interval"
    continue
  fi

  if ! /usr/bin/node "$repo/contracts/scripts/tap-deposit-watcher.mjs" \
    --rpc "$MAYHEM_TAP_ETH_RPC" \
    --pool "$pool" \
    --peer-rpc "$peer_rpc" \
    --cursor "$cursor" \
    --confirmations 12 \
    --admin-home "$home" \
    --mayhem-bin "$mayhem_bin" \
    --submit \
    --json; then
    echo "TAP deposit watcher tick failed; retrying after ${interval}s" >&2
  fi
  sleep "$interval"
done
