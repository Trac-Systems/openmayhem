#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_SCRIPT="$ROOT_DIR/scripts/tests/windows-source-build-environment.test.ps1"
PWSH_IMAGE="${MAYHEM_DOCKER_PWSH_IMAGE:-mcr.microsoft.com/powershell:7.4-debian-12}"
STAGED_ROOT=""

cleanup() {
  if [[ -n "$STAGED_ROOT" ]]; then
    rm -rf "$STAGED_ROOT"
  fi
}
trap cleanup EXIT

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoProfile -NonInteractive \
    -File "$TEST_SCRIPT" \
    -InstallerPath "$ROOT_DIR/install.ps1"
  exit
fi

if command -v docker >/dev/null 2>&1; then
  WORK_ROOT="$ROOT_DIR"
  if ! docker run --rm \
    -v "$ROOT_DIR:/work:ro" \
    "$PWSH_IMAGE" \
    pwsh -NoProfile -NonInteractive \
    -Command 'if (-not (Test-Path /work/install.ps1)) { exit 1 }' \
    >/dev/null 2>&1; then
    command -v rsync >/dev/null 2>&1 || {
      printf 'windows-source-build-environment.test: rsync is required for Docker staging\n' >&2
      exit 1
    }
    STAGED_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-windows-build-env.XXXXXX")"
    mkdir -p "$STAGED_ROOT/scripts/tests"
    rsync -a \
      "$ROOT_DIR/install.ps1" \
      "$STAGED_ROOT/install.ps1"
    rsync -a \
      "$ROOT_DIR/scripts/tests/windows-source-build-environment.test.ps1" \
      "$STAGED_ROOT/scripts/tests/windows-source-build-environment.test.ps1"
    WORK_ROOT="$STAGED_ROOT"
  fi

  docker run --rm \
    -v "$WORK_ROOT:/work:ro" \
    "$PWSH_IMAGE" \
    pwsh -NoProfile -NonInteractive \
    -File /work/scripts/tests/windows-source-build-environment.test.ps1 \
    -InstallerPath /work/install.ps1
  exit
fi

printf 'windows-source-build-environment.test: pwsh or docker is required\n' >&2
exit 1
