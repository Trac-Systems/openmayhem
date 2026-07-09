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
paths=(opt/mayhem/.mayhem-local)
while IFS= read -r unit; do paths+=("${unit#/}"); done < <(
  find /etc/systemd/system -maxdepth 1 -type f \
    \( -name 'mayhem-*.service' -o -name 'mayhem-*.timer' \) -print | sort
)

tar -czf "$temporary" -C / "${paths[@]}"
tar -tzf "$temporary" >/dev/null
chmod 600 "$temporary"
mv "$temporary" "$archive"

mapfile -t expired < <(find "$backup_dir" -maxdepth 1 -type f -name 'mayhem-mainnet-*.tar.gz' -printf '%T@ %p\n' \
  | sort -rn | awk -v keep="$retention" 'NR > keep {sub(/^[^ ]+ /, ""); print}')
if (( ${#expired[@]} > 0 )); then rm -f -- "${expired[@]}"; fi

echo "$archive"
