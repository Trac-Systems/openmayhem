#!/usr/bin/env bash
set -euo pipefail

effective_uid="${EUID:-$(id -u)}"
[[ "$effective_uid" =~ ^(0|[1-9][0-9]*)$ ]] || {
  echo "Could not determine a canonical effective uid." >&2
  exit 1
}
if [[ "$effective_uid" != "0" ]]; then
  echo "Run as root: sudo scripts/install-mainnet-systemd.sh" >&2
  exit 1
fi

root="${MAYHEM_ROOT:-/opt/mayhem}"
repo="${MAYHEM_REPO:-$root/source}"
if [[ "$root" != "/opt/mayhem" || "$repo" != "/opt/mayhem/source" ]]; then
  echo "Refusing mainnet systemd install: canonical units require MAYHEM_ROOT=/opt/mayhem and MAYHEM_REPO=/opt/mayhem/source." >&2
  exit 1
fi
secrets="$root/.mayhem-local/secrets/GO-LIVE"
live_env="$secrets/00-mayhem-live.env"
overlay_env="$secrets/10-mayhem-stack.env"
systemd_env="$secrets/mayhem-systemd.env"
systemd_env_tmp="$systemd_env.tmp"

cleanup() {
  rm -f "$systemd_env_tmp"
}
trap cleanup EXIT

for path in \
  "$live_env" \
  "$overlay_env" \
  "$repo/ops/systemd/mayhem-stack.service" \
  "$repo/ops/systemd/mayhem-payout-worker.service" \
  "$repo/ops/systemd/mayhem-payout-worker.timer" \
  "$repo/scripts/ops/backup-mainnet.sh"; do
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
append_default MAYHEM_ASSET_DIR "$repo"
append_default MAYHEM_PEER_RPC 'http://127.0.0.1:49223/v1'
append_default MAYHEM_CONTRACT_RPC_URL 'http://127.0.0.1:49223/v1'
append_default MAYHEM_RPC_URL 'http://127.0.0.1:49223/v1'
append_default MAYHEM_GATEWAY_URL 'http://127.0.0.1:11435'
append_default MAYHEM_ADMIN_HOME "$root/.mayhem-local/live-home"
append_default MAYHEM_CADENCE_STATE_DIR "$root/.mayhem-local/settlement"
append_default MAYHEM_SOURCE_DIR "$repo"
append_default MAYHEM_TAP_SETTLEMENT_SPOOL "$root/.mayhem-local/settlement/tap"
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
append_default MAYHEM_FIAT_TRANSFER_MAX_ATTEMPTS '4'
append_default MAYHEM_FIAT_TRANSFER_RETRY_MS '500'
append_default MAYHEM_TAP_SETTLEMENT_ENABLED '1'
append_default MAYHEM_TAP_SETTLEMENT_ATTEMPT_TIMEOUT_SECONDS '900'
append_default MAYHEM_TNK_SETTLEMENT_ENABLED '1'
append_default MAYHEM_TNK_TRANSFER_TIMEOUT_SECONDS '180'
append_default MAYHEM_TNK_TRANSFER_MAX_RETRIES '3'
append_default MAYHEM_PAYOUT_MAX_ATTEMPTS '8'
append_default MAYHEM_PAYOUT_RETRY_BACKOFF_SECONDS '300'
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

require_present() {
  local key="$1"
  [[ -n "$(env_value "$key")" ]] || {
    echo "Refusing live install: $key is required by the automatic payout worker." >&2
    exit 1
  }
}

require_private_key() {
  local key="$1" value
  value="$(env_value "$key")"
  [[ "$value" =~ ^0x[0-9a-fA-F]{64}$ ]] || {
    echo "Refusing live install: $key must be a 0x-prefixed 32-byte private key." >&2
    exit 1
  }
}

require_eth_address() {
  local key="$1" value
  value="$(env_value "$key")"
  [[ "$value" =~ ^0x[0-9a-fA-F]{40}$ ]] || {
    echo "Refusing live install: $key must be a 20-byte Ethereum address." >&2
    exit 1
  }
}

require_file_env() {
  local key="$1" value
  value="$(env_value "$key")"
  [[ -f "$value" ]] || {
    echo "Refusing live install: $key does not name an existing file." >&2
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
require_equal MAYHEM_TAP_POOL_ADDRESS '0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28'
require_equal MAYHEM_TAP_GOVERNANCE_SIGNER '0x199E54a7dfe9DcfD4567Fd635a298e2902d0F8B3'
require_equal MAYHEM_TAP_GOVERNANCE_DELAY_SECONDS '3600'
require_live_rpc MAYHEM_TAP_ETH_RPC
require_live_rpc MAYHEM_TAP_ETH_RPC_FALLBACKS
require_live_rpc MAYHEM_TAP_ETH_PUBLIC_RPC_FALLBACKS
require_equal MAYHEM_TAP_PRICE_FLOOR_AU '1000000000000000'
require_equal MAYHEM_TAP_PRICE_CEILING_AU '10000000000000000000'
require_equal MAYHEM_TAP_PRICE_MAX_DEVIATION_BPS '2000'
require_equal MAYHEM_TAP_PRICE_MIN_SOURCES '2'
require_equal MAYHEM_TAP_TWAP_WINDOW_SECONDS '1800'
require_equal MAYHEM_PAYGATE_STRIPE_ENABLED '1'
require_equal MAYHEM_STRIPE_MODE 'live'
require_equal MAYHEM_STRIPE_API_BASE_URL 'https://api.stripe.com'
require_prefix MAYHEM_STRIPE_SECRET_KEY 'sk_live_'
require_equal MAYHEM_FIAT_SETTLEMENT_ENABLED '1'
require_equal MAYHEM_TAP_SETTLEMENT_ENABLED '1'
require_equal MAYHEM_TNK_SETTLEMENT_ENABLED '1'
require_eth_address MAYHEM_TAP_OPERATOR_ADDRESS
require_private_key MAYHEM_TAP_ROLLER_PRIVATE_KEY
require_private_key MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY
[[ "$(env_value MAYHEM_TAP_ROLLER_PRIVATE_KEY)" != "$(env_value MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY)" ]] || {
  echo "Refusing live install: TAP owner and governance payout signers must be distinct." >&2
  exit 1
}
require_present MAYHEM_TNK_OPERATOR_ADDRESS
require_present MAYHEM_TNK_TREASURY_KEYPAIR_PATH
require_file_env MAYHEM_TNK_TREASURY_KEYPAIR_PATH

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

hydrate_intercom_package() {
  local dir="$repo/intercom"
  local verifier="$repo/scripts/verify-intercom-dependency-topology.mjs"
  local materializer="$dir/scripts/materialize-local-dependencies.mjs"

  [[ -f "$dir/package.json" ]] || {
    echo "Missing Intercom runtime manifest: $dir/package.json" >&2
    exit 1
  }
  [[ -f "$dir/package-lock.json" ]] || {
    echo "Missing Intercom root dependency lock: $dir/package-lock.json" >&2
    exit 1
  }
  [[ -f "$dir/.npmrc" ]] || {
    echo "Missing Intercom root npm configuration: $dir/.npmrc" >&2
    exit 1
  }
  [[ -f "$verifier" ]] || {
    echo "Missing Intercom dependency topology verifier: $verifier" >&2
    exit 1
  }
  [[ -f "$materializer" ]] || {
    echo "Missing Intercom local dependency materializer: $materializer" >&2
    exit 1
  }

  echo "Installing root-authoritative runtime dependencies in $dir."
  rm -rf "$dir/trac/msb/node_modules" "$dir/trac/trac-peer/node_modules"
  runuser -u mayhem -- npm ci --omit=dev --install-links=true --prefix "$dir"
  runuser -u mayhem -- node "$materializer" "$dir"
  runuser -u mayhem -- node "$verifier" "$dir"
}

command -v npm >/dev/null 2>&1 || {
  echo "npm is required to install the mainnet runtime dependencies." >&2
  exit 1
}
command -v runuser >/dev/null 2>&1 || {
  echo "runuser is required to install dependencies as the mayhem user." >&2
  exit 1
}
command -v flock >/dev/null 2>&1 || {
  echo "flock is required to serialize automatic epoch payout work." >&2
  exit 1
}
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required by the automatic epoch payout worker." >&2
  exit 1
}
command -v jq >/dev/null 2>&1 || {
  echo "jq is required by the automatic TAP payout worker." >&2
  exit 1
}
command -v timeout >/dev/null 2>&1 || {
  echo "timeout is required to bound automatic TAP payout attempts." >&2
  exit 1
}

hydrate_intercom_package
hydrate_npm_package "$repo/contracts"

install -d -m 700 -o mayhem -g mayhem \
  "$root/backups" \
  "$root/.mayhem-local/paygate" \
  "$root/.mayhem-local/watchers" \
  "$root/.mayhem-local/settlement/epochs" \
  "$root/.mayhem-local/settlement/payout" \
  "$root/.mayhem-local/settlement/tap/ready" \
  "$root/.mayhem-local/settlement/tap/working" \
  "$root/.mayhem-local/settlement/tap/processed" \
  "$root/.mayhem-local/settlement/tap/failed"
install -d -m 0750 -o root -g root "$root/libexec"
install -m 0750 -o root -g root \
  "$repo/scripts/ops/backup-mainnet.sh" \
  "$root/libexec/backup-mainnet.sh"
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
  mayhem-epoch-cadence.timer \
  mayhem-backup.timer
systemctl enable --now mayhem-payout-worker.timer

echo "Mayhem mainnet units installed; the automatic payout timer is enabled and running."
