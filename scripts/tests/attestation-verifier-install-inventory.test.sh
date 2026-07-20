#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
failures=0

check_shell_inventory() {
  local relative="$1"
  local block count

  block="$(sed -n '/^BINS=(/,/^)/p' "$ROOT_DIR/$relative")"
  count="$(grep -c '^  mayhem-attestation-verifier$' <<<"$block" || true)"
  if [[ "$count" != "1" ]]; then
    printf 'attestation-verifier-install-inventory.test: %s must list the verifier exactly once\n' \
      "$relative" >&2
    failures=$((failures + 1))
  fi
}

check_powershell_inventory() {
  local relative="$1"
  local block count

  block="$(sed -n '/^\$Bins = @(/,/^)/p' "$ROOT_DIR/$relative")"
  count="$(
    grep -Ec '^[[:space:]]*"mayhem-attestation-verifier",[[:space:]]*$' \
      <<<"$block" || true
  )"
  if [[ "$count" != "1" ]]; then
    printf 'attestation-verifier-install-inventory.test: %s must list the verifier exactly once\n' \
      "$relative" >&2
    failures=$((failures + 1))
  fi
}

check_shell_inventory scripts/package-release.sh
check_shell_inventory install.sh
check_shell_inventory scripts/macos-install-check.sh
check_shell_inventory scripts/docker-linux-install-check.sh
check_powershell_inventory install.ps1

if ! grep -Fq 'local artifact_version="${VERSION#v}"' "$ROOT_DIR/install.sh"; then
  printf 'attestation-verifier-install-inventory.test: install.sh must remove the Git tag prefix from artifact names\n' >&2
  failures=$((failures + 1))
fi

if ! grep -Fq '$artifactVersion = $Version -replace "^v", ""' "$ROOT_DIR/install.ps1"; then
  printf 'attestation-verifier-install-inventory.test: install.ps1 must remove the Git tag prefix from artifact names\n' >&2
  failures=$((failures + 1))
fi

if [[ "$failures" != "0" ]]; then
  exit 1
fi

printf 'attestation-verifier-install-inventory.test: ok\n'
