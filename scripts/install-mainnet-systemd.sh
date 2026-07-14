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
systemd_env_tmp="$systemd_env.tmp"

cleanup() {
  rm -f "$systemd_env_tmp"
}
trap cleanup EXIT

for path in "$live_env" "$overlay_env" "$repo/ops/systemd/mayhem-stack.service"; do
  [[ -f "$path" ]] || { echo "Missing required file: $path" >&2; exit 1; }
done

umask 077
awk '
  /^(export )?[A-Za-z_][A-Za-z0-9_]*=/ {
    sub(/^export /, "")
    key = $0
    sub(/=.*/, "", key)
    if (!(key in seen)) {
      order[++count] = key
      seen[key] = 1
    }
    value[key] = $0
  }
  END {
    for (i = 1; i <= count; i++) print value[order[i]]
  }
' "$live_env" "$overlay_env" > "$systemd_env_tmp"

append_default() {
  local key="$1" value="$2"
  if ! grep -q "^${key}=" "$systemd_env_tmp"; then
    printf "%s='%s'\n" "$key" "$value" >> "$systemd_env_tmp"
  fi
}

append_default MAYHEM_REPO "$repo"
append_default MAYHEM_PEER_RPC 'http://127.0.0.1:49223/v1'
append_default MAYHEM_CONTRACT_RPC_URL 'http://127.0.0.1:49223/v1'
append_default MAYHEM_PAYGATE_BIND '127.0.0.1:11436'
append_default MAYHEM_PAYGATE_CONTRACT_DRY_RUN '0'
append_default MAYHEM_PAYGATE_STRIPE_EVENTS_PATH "$root/.mayhem-local/paygate/stripe-events.jsonl"
append_default MAYHEM_STRIPE_CONNECT_ACCOUNT_TYPE 'express'
append_default MAYHEM_STRIPE_CONNECT_ACCOUNTS_PATH "$root/.mayhem-local/paygate/stripe-connect-accounts.jsonl"
append_default MAYHEM_STRIPE_CONNECT_RETURN_URL 'https://dashboard.stripe.com/'
append_default MAYHEM_STRIPE_CONNECT_REFRESH_URL 'https://dashboard.stripe.com/'
append_default MAYHEM_STRIPE_BACKFILL_ENABLED '1'
append_default MAYHEM_STRIPE_BACKFILL_CURSOR_PATH "$root/.mayhem-local/paygate/stripe-backfill-cursor.json"
append_default MAYHEM_STRIPE_BACKFILL_INTERVAL_SECONDS '300'
append_default MAYHEM_FIAT_SETTLEMENT_ENABLED '1'
append_default MAYHEM_FIAT_OPERATOR_ACCOUNT 'platform_balance'
append_default MAYHEM_FIAT_OPERATOR_CURRENCY 'eur'
append_default MAYHEM_ADMIN_WALLET_PASSWORD ''
append_default MAYHEM_WALLET_PASSWORD ''
append_default MAYHEM_TNK_WALLET_PASSWORD ''
append_default MAYHEM_MSB_NETWORK 'mainnet'

env_value() {
  local key="$1" value
  value="$(awk -F= -v key="$key" '$1 == key { value = substr($0, index($0, "=") + 1) } END { print value }' "$systemd_env_tmp")"
  if [[ "$value" == \'*\' || "$value" == \"*\" ]]; then
    value="${value:1:${#value}-2}"
  fi
  printf '%s' "$value"
}

require_equal() {
  local key="$1" expected="$2"
  [[ "$(env_value "$key")" == "$expected" ]] || {
    echo "Refusing live install: $key is not the canonical mainnet value." >&2
    exit 1
  }
}

require_prefix() {
  local key="$1" prefix="$2" value
  value="$(env_value "$key")"
  [[ "$value" == "$prefix"* ]] || {
    echo "Refusing live install: $key is missing or has the wrong live-mode prefix." >&2
    exit 1
  }
}

require_live_rpc() {
  local key="$1" value
  value="$(env_value "$key")"
  [[ "$value" == https://* ]] || {
    echo "Refusing live install: $key must contain an HTTPS mainnet endpoint." >&2
    exit 1
  }
  [[ ! "$value" =~ (localhost|127\.0\.0\.1|testnet|sepolia|goerli|holesky) ]] || {
    echo "Refusing live install: $key contains a non-mainnet endpoint." >&2
    exit 1
  }
}

require_equal MAYHEM_NETWORK 'mainnet'
require_equal MAYHEM_MSB_NETWORK 'mainnet'
require_equal MSB_BOOTSTRAP 'acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685'
require_equal MSB_CHANNEL '0000trac0network0msb0mainnet0000'
require_equal MAYHEM_TNK_TREASURY_ADDRESS 'trac1f3w8ja3qxcnmzzmxxt8m0ystdf683sy5arnhxvz0h7a8ydd0kqwq3lcgdh'
require_equal MAYHEM_TAP_ETH_CHAIN_ID '1'
require_equal MAYHEM_TAP_TOKEN_ADDR '0x5e7F6e008C6d9D7AD4c7EB75Bd4ce62864cc7454'
require_equal MAYHEM_TAP_POOL_ADDRESS '0x9B254d37C28Fb5893F46513a61925eDC2F300615'
require_live_rpc MAYHEM_TAP_ETH_RPC
require_live_rpc MAYHEM_TAP_ETH_RPC_FALLBACKS
require_equal MAYHEM_PAYGATE_STRIPE_ENABLED '1'
require_equal MAYHEM_STRIPE_MODE 'live'
require_equal MAYHEM_STRIPE_API_BASE_URL 'https://api.stripe.com'
require_prefix MAYHEM_STRIPE_SECRET_KEY 'sk_live_'

mv "$systemd_env_tmp" "$systemd_env"
chmod 600 "$systemd_env"

if [[ "${MAYHEM_RENDER_ONLY:-0}" == "1" ]]; then
  echo "Canonical mainnet payment environment validated at $systemd_env."
  exit 0
fi

hydrate_npm_package() {
  local dir="$1"

  [[ -f "$dir/package.json" ]] || {
    echo "Missing runtime package manifest: $dir/package.json" >&2
    exit 1
  }
  [[ -f "$dir/package-lock.json" ]] || {
    echo "Missing runtime dependency lock: $dir/package-lock.json" >&2
    exit 1
  }

  echo "Installing locked runtime dependencies in $dir."
  runuser -u mayhem -- npm ci --omit=dev --prefix "$dir"
}

command -v npm >/dev/null 2>&1 || {
  echo "npm is required to install the mainnet runtime dependencies." >&2
  exit 1
}
command -v runuser >/dev/null 2>&1 || {
  echo "runuser is required to install dependencies as the mayhem user." >&2
  exit 1
}

hydrate_npm_package "$repo/intercom/trac/msb"
hydrate_npm_package "$repo/intercom/trac/trac-peer"
hydrate_npm_package "$repo/intercom"
hydrate_npm_package "$repo/contracts"

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
