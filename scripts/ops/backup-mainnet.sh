#!/usr/bin/env bash
set -euo pipefail

root="${MAYHEM_ROOT:-/opt/mayhem}"
backup_dir="${MAYHEM_BACKUP_DIR:-$root/backups}"
retention="${MAYHEM_BACKUP_RETENTION:-14}"

mkdir -p "$backup_dir"
chmod 700 "$backup_dir"

if [[ "${1:-}" == "--restore-drill" ]]; then
  archive="${2:?usage: backup-mainnet.sh --restore-drill <archive>}"
  drill="$backup_dir/.restore-drill-$$"
  trap 'rm -rf "$drill"' EXIT
  mkdir -p "$drill"
  tar -xzf "$archive" -C "$drill"
  test -d "$drill/opt/mayhem/.mayhem-local"
  test -f "$drill/opt/mayhem/.mayhem-local/live-home/config.toml"
  test -d "$drill/opt/mayhem/.mayhem-local/live-home/stores"
  while IFS= read -r file; do jq empty "$file" >/dev/null; done < <(
    find "$drill/opt/mayhem/.mayhem-local" -type f \
      \( -name '*cursor*.json' -o -name '*watcher*.json' \) -print
  )
  echo "Backup restore drill passed: $(basename "$archive")"
  exit 0
fi

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
archive="$backup_dir/mayhem-mainnet-$timestamp.tar.gz"
temporary="$archive.partial"
lock_file="$backup_dir/.backup-mainnet.lock"
quiesced_units=(
  mayhem-payout-worker.timer
  mayhem-epoch-cadence.timer
  mayhem-payout-worker.service
  mayhem-epoch-cadence.service
  mayhem-paygate.service
  mayhem-tap-rate.service
  mayhem-tnk-rate.service
  mayhem-tap-deposit.service
  mayhem-tnk-deposit.service
  mayhem-tap-settlement.service
  mayhem-stack.service
)
systemd_unit_paths_output=""
if ! systemd_unit_paths_output="$(
  find /etc/systemd/system -maxdepth 1 -type f \
    \( -name 'mayhem-*.service' -o -name 'mayhem-*.timer' \) -print | sort
)"; then
  echo "Could not inventory installed Mayhem systemd units." >&2
  exit 1
fi
systemd_unit_paths=()
paths=(opt/mayhem/.mayhem-local)
if [[ -n "$systemd_unit_paths_output" ]]; then
  while IFS= read -r unit_path; do
    systemd_unit_paths+=("$unit_path")
    paths+=("${unit_path#/}")
  done <<<"$systemd_unit_paths_output"
fi
for unit in "${quiesced_units[@]}"; do
  found=0
  for unit_path in "${systemd_unit_paths[@]}"; do
    if [[ "$unit_path" == "/etc/systemd/system/$unit" ]]; then
      found=1
      break
    fi
  done
  if (( found == 0 )); then
    echo "Missing tracked Mayhem systemd unit: $unit" >&2
    exit 1
  fi
done

initial_states=()
unit_state=""
restore_required=0
restoration_attempted=0
restoration_complete=0

query_unit_state() {
  local unit="$1" output status

  if output="$(systemctl is-active "$unit")"; then
    status=0
  else
    status=$?
  fi

  case "$status:$output" in
    0:active) unit_state=active ;;
    3:inactive) unit_state=inactive ;;
    *)
      echo "Could not determine active/inactive state for $unit (status=$status, state=${output:-unknown})." >&2
      return 1
      ;;
  esac
}

restore_original_state() {
  local failed=0 index unit expected

  restoration_attempted=1
  for ((index = ${#quiesced_units[@]} - 1; index >= 0; index -= 1)); do
    if [[ "${initial_states[index]}" == "active" ]]; then
      unit="${quiesced_units[index]}"
      if ! systemctl start "$unit"; then
        echo "Failed to restore active unit: $unit" >&2
        failed=1
      fi
    fi
  done

  for ((index = 0; index < ${#quiesced_units[@]}; index += 1)); do
    [[ "${initial_states[index]}" == "inactive" ]] || continue
    unit="${quiesced_units[index]}"
    if ! query_unit_state "$unit"; then
      failed=1
    elif [[ "$unit_state" == "active" ]] && ! systemctl stop "$unit"; then
      echo "Failed to return inactive unit to its original state: $unit" >&2
      failed=1
    fi
  done

  for ((index = 0; index < ${#quiesced_units[@]}; index += 1)); do
    unit="${quiesced_units[index]}"
    expected="${initial_states[index]}"
    if ! query_unit_state "$unit"; then
      failed=1
    elif [[ "$unit_state" != "$expected" ]]; then
      echo "Unit state restoration mismatch for $unit: expected $expected, found $unit_state." >&2
      failed=1
    fi
  done

  if (( failed != 0 )); then
    return 1
  fi
  restoration_complete=1
  restore_required=0
}

cleanup_backup() {
  local status=$?

  trap - EXIT
  rm -f -- "$temporary"
  if (( restore_required == 1 && restoration_attempted == 0 )); then
    if ! restore_original_state; then
      echo "Backup failed and the original unit state could not be fully restored." >&2
      status=1
    fi
  elif (( restore_required == 1 && restoration_complete == 0 )); then
    status=1
  fi
  exit "$status"
}
trap cleanup_backup EXIT

exec 9>"$lock_file"
if ! flock -n 9; then
  echo "Another mainnet backup is already running." >&2
  exit 1
fi

for unit in "${quiesced_units[@]}"; do
  query_unit_state "$unit"
  initial_states+=("$unit_state")
done

restore_required=1
for unit in "${quiesced_units[@]}"; do
  systemctl stop "$unit"
done
for unit in "${quiesced_units[@]}"; do
  query_unit_state "$unit"
  if [[ "$unit_state" != "inactive" ]]; then
    echo "Refusing backup because $unit did not quiesce." >&2
    exit 1
  fi
done

tar -czf "$temporary" -C / "${paths[@]}"
[[ -s "$temporary" ]] || {
  echo "Backup archive is empty: $temporary" >&2
  exit 1
}
tar -tzf "$temporary" >/dev/null
chmod 600 "$temporary"
mv "$temporary" "$archive"

if ! restore_original_state; then
  echo "Backup archive was verified, but the original unit state was not fully restored." >&2
  exit 1
fi

expired=()
while IFS= read -r expired_archive; do
  expired+=("$expired_archive")
done < <(find "$backup_dir" -maxdepth 1 -type f -name 'mayhem-mainnet-*.tar.gz' -printf '%T@ %p\n' \
  | sort -rn | awk -v keep="$retention" 'NR > keep {sub(/^[^ ]+ /, ""); print}')
if (( ${#expired[@]} > 0 )); then rm -f -- "${expired[@]}"; fi

echo "$archive"
