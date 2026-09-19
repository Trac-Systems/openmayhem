#!/usr/bin/env bash
set -euo pipefail

if [[ ${EUID:-$(id -u)} -ne 0 ]]; then
  echo "Run as root: sudo scripts/ops/install-retail-crypto-payment-worker.sh" >&2
  exit 1
fi

repo="${MAYHEM_REPO:-/opt/mayhem/source}"
root="${MAYHEM_ROOT:-/opt/mayhem}"
service_user="${MAYHEM_SERVICE_USER:-mayhem}"
service_group="${MAYHEM_SERVICE_GROUP:-$service_user}"
buyer_home="${MAYHEM_BUYER_HOME:-$root/.mayhem-local/live-home}"
state_dir="${MAYHEM_WORKER_STATE_DIR:-$root/.mayhem-local/retail-crypto-worker}"
secret="${MAYHEM_WORKER_ENV:-$root/.mayhem-local/secrets/GO-LIVE/retail-crypto-payment-worker.env}"
system_env="${MAYHEM_SYSTEM_ENV:-$root/.mayhem-local/secrets/GO-LIVE/mayhem-systemd.env}"
template="$repo/ops/systemd/mayhem-retail-crypto-payment-worker.service"

for value in "$repo" "$root" "$service_user" "$service_group" "$buyer_home" "$state_dir" "$secret" "$system_env"; do
  [[ "$value" != *$'\n'* && "$value" != *$'\r'* ]] || {
    echo "Service configuration values must not contain newlines." >&2
    exit 1
  }
done
id "$service_user" >/dev/null 2>&1 || {
  echo "Unknown service user: $service_user" >&2
  exit 1
}
getent group "$service_group" >/dev/null 2>&1 || {
  echo "Unknown service group: $service_group" >&2
  exit 1
}

[[ -f "$secret" ]] || {
  echo "Missing protected worker environment: $secret" >&2
  exit 1
}
[[ "$(stat -c '%a' "$secret")" == "600" ]] || {
  echo "Worker environment must have mode 0600: $secret" >&2
  exit 1
}

/usr/bin/node --check "$repo/intercom/scripts/retail-crypto-payment-worker.mjs"
install -d -m 0700 -o "$service_user" -g "$service_group" "$state_dir"

unit="$(cat "$template")"
unit="${unit//@@SERVICE_USER@@/$service_user}"
unit="${unit//@@SERVICE_GROUP@@/$service_group}"
unit="${unit//@@REPO@@/$repo}"
unit="${unit//@@SYSTEM_ENV@@/$system_env}"
unit="${unit//@@WORKER_ENV@@/$secret}"
unit="${unit//@@STATE_DIR@@/$state_dir}"
unit="${unit//@@BUYER_HOME@@/$buyer_home}"
printf '%s\n' "$unit" > /etc/systemd/system/mayhem-retail-crypto-payment-worker.service
chmod 0644 /etc/systemd/system/mayhem-retail-crypto-payment-worker.service
systemd-analyze verify /etc/systemd/system/mayhem-retail-crypto-payment-worker.service
systemctl daemon-reload
systemctl enable --now mayhem-retail-crypto-payment-worker.service
systemctl is-active --quiet mayhem-retail-crypto-payment-worker.service
echo "Retail crypto payment worker is active."
