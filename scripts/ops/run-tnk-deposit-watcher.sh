#!/usr/bin/env bash
set -euo pipefail

repo="${MAYHEM_REPO:-/opt/mayhem/source}"
home="${MAYHEM_HOME:-/opt/mayhem/.mayhem-local/live-home}"
peer_rpc="${MAYHEM_PEER_RPC:-http://127.0.0.1:49223/v1}"
cursor="${MAYHEM_TNK_DEPOSIT_CURSOR:-/opt/mayhem/.mayhem-local/watchers/tnk-deposit.json}"
state_dir="${MAYHEM_TNK_DEPOSIT_STATE_DIR:-/opt/mayhem/.mayhem-local/tnk-deposit-msb}"
store_name="${MAYHEM_TNK_DEPOSIT_STORE_NAME:-mayhem-mainnet-deposit-watcher}"
interval="${MAYHEM_TNK_DEPOSIT_INTERVAL_SECONDS:-30}"

mkdir -p "$(dirname "$cursor")" "$state_dir"

while true; do
  if ! /usr/bin/node "$repo/intercom/scripts/tnk-deposit-watcher.mjs" \
    --network mainnet \
    --state-dir "$state_dir" \
    --store-name "$store_name" \
    --peer-rpc "$peer_rpc" \
    --cursor "$cursor" \
    --admin-home "$home" \
    --mayhem-bin "$repo/target/release/mayhem" \
    --submit \
    --json; then
    echo "TNK deposit watcher tick failed; retrying after ${interval}s" >&2
  fi
  sleep "$interval"
done
