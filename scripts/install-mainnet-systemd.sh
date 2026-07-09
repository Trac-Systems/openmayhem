#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "Run as root: sudo scripts/install-mainnet-systemd.sh" >&2
  exit 1
fi

root="${MAYHEM_ROOT:-/opt/mayhem}"
repo="${MAYHEM_REPO:-$root/source}"
secrets="$root/.mayhem-local/secrets/GO-LIVE"
live_env="$secrets/00-mayhem-live.env"
overlay_env="$secrets/10-mayhem-stack.env"
systemd_env="$secrets/mayhem-systemd.env"

for path in "$live_env" "$overlay_env" "$repo/ops/systemd/mayhem-stack.service"; do
  [[ -f "$path" ]] || { echo "Missing required file: $path" >&2; exit 1; }
done

umask 077
awk '/^export [A-Za-z_][A-Za-z0-9_]*=/{sub(/^export /, ""); print}' "$live_env" > "$systemd_env.tmp"
awk '/^[A-Za-z_][A-Za-z0-9_]*=/{print}' "$overlay_env" >> "$systemd_env.tmp"
cat >> "$systemd_env.tmp" <<'ENV'
MAYHEM_REPO='/opt/mayhem/source'
MAYHEM_PEER_RPC='http://127.0.0.1:49223/v1'
MAYHEM_CONTRACT_RPC_URL='http://127.0.0.1:49223/v1'
MAYHEM_PAYGATE_BIND='127.0.0.1:11436'
MAYHEM_PAYGATE_CONTRACT_DRY_RUN='0'
MAYHEM_PAYGATE_STRIPE_EVENTS_PATH='/opt/mayhem/.mayhem-local/paygate/stripe-events.jsonl'
MAYHEM_STRIPE_BACKFILL_ENABLED='1'
MAYHEM_STRIPE_BACKFILL_CURSOR_PATH='/opt/mayhem/.mayhem-local/paygate/stripe-backfill-cursor.json'
MAYHEM_STRIPE_BACKFILL_INTERVAL_SECONDS='300'
MAYHEM_ADMIN_WALLET_PASSWORD=''
MAYHEM_WALLET_PASSWORD=''
MAYHEM_TNK_WALLET_PASSWORD=''
MAYHEM_MSB_NETWORK='mainnet'
ENV
mv "$systemd_env.tmp" "$systemd_env"
chmod 600 "$systemd_env"

install -d -m 700 -o mayhem -g mayhem \
  "$root/backups" \
  "$root/.mayhem-local/paygate" \
  "$root/.mayhem-local/watchers" \
  "$root/.mayhem-local/settlement/tap/ready" \
  "$root/.mayhem-local/settlement/tap/working" \
  "$root/.mayhem-local/settlement/tap/processed" \
  "$root/.mayhem-local/settlement/tap/failed"
chown mayhem:mayhem "$systemd_env"

install -m 0644 "$repo"/ops/systemd/mayhem-*.service /etc/systemd/system/
install -m 0644 "$repo"/ops/systemd/mayhem-*.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable \
  mayhem-stack.service \
  mayhem-paygate.service \
  mayhem-tap-rate.service \
  mayhem-tnk-rate.service \
  mayhem-tap-deposit.service \
  mayhem-tnk-deposit.service \
  mayhem-tap-settlement.service \
  mayhem-backup.timer

echo "Mayhem mainnet units installed and enabled."
