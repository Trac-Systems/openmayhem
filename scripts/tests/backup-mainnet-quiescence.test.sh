#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BACKUP_SCRIPT="$ROOT_DIR/scripts/ops/backup-mainnet.sh"

UNITS=(
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
ACTIVE_UNITS=(
  mayhem-payout-worker.timer
  mayhem-epoch-cadence.timer
  mayhem-paygate.service
  mayhem-tnk-rate.service
  mayhem-tap-deposit.service
  mayhem-stack.service
)
RESTORE_UNITS=(
  mayhem-stack.service
  mayhem-tap-deposit.service
  mayhem-tnk-rate.service
  mayhem-paygate.service
  mayhem-epoch-cadence.timer
  mayhem-payout-worker.timer
)

fail() {
  printf 'backup-mainnet-quiescence.test: %s\n' "$*" >&2
  exit 1
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-backup-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mock_bin="$tmp/mock-bin"
mkdir -p "$mock_bin"

cat >"$mock_bin/date" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == "-u +%Y%m%dT%H%M%SZ" ]] || exit 1
printf '%s\n' '20260720T120000Z'
MOCK

cat >"$mock_bin/find" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  /etc/systemd/system)
    printf '%s\n' \
      /etc/systemd/system/mayhem-backup.service \
      /etc/systemd/system/mayhem-backup.timer \
      /etc/systemd/system/mayhem-epoch-cadence.service \
      /etc/systemd/system/mayhem-epoch-cadence.timer \
      /etc/systemd/system/mayhem-paygate.service \
      /etc/systemd/system/mayhem-payout-worker.service \
      /etc/systemd/system/mayhem-payout-worker.timer \
      /etc/systemd/system/mayhem-stack.service \
      /etc/systemd/system/mayhem-tap-deposit.service \
      /etc/systemd/system/mayhem-tap-rate.service \
      /etc/systemd/system/mayhem-tap-settlement.service \
      /etc/systemd/system/mayhem-tnk-deposit.service \
      /etc/systemd/system/mayhem-tnk-rate.service
    ;;
  "$MAYHEM_BACKUP_DIR")
    ;;
  *)
    printf 'unexpected find invocation: %s\n' "$*" >&2
    exit 1
    ;;
esac
MOCK

cat >"$mock_bin/flock" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
[[ "$#" == "2" && "$1" == "-n" && "$2" == "9" ]]
MOCK

cat >"$mock_bin/tar" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  -czf)
    printf 'tar-copy'
    printf ' %s' "$@"
    printf '\n'
    printf 'mock archive\n' >"$2"
    [[ "${MAYHEM_MOCK_TAR_CREATE_FAIL:-0}" != "1" ]]
    ;;
  -tzf)
    printf 'tar-verify %s\n' "$2"
    [[ -s "$2" ]]
    [[ "${MAYHEM_MOCK_TAR_VERIFY_FAIL:-0}" != "1" ]]
    ;;
  *)
    printf 'unexpected tar invocation: %s\n' "$*" >&2
    exit 1
    ;;
esac >>"$MAYHEM_MOCK_LOG"
MOCK

cat >"$mock_bin/systemctl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

state_for() {
  awk -v unit="$1" '$1 == unit { print $2; found = 1 } END { if (!found) exit 1 }' \
    "$MAYHEM_MOCK_STATE"
}

set_state() {
  local unit="$1" state="$2"
  awk -v unit="$unit" '$1 != unit { print }' "$MAYHEM_MOCK_STATE" \
    >"$MAYHEM_MOCK_STATE.tmp"
  printf '%s %s\n' "$unit" "$state" >>"$MAYHEM_MOCK_STATE.tmp"
  mv "$MAYHEM_MOCK_STATE.tmp" "$MAYHEM_MOCK_STATE"
}

command="${1:-}"
shift || true
case "$command" in
  is-active)
    [[ "$#" == "1" ]]
    state="$(state_for "$1")" || {
      printf '%s\n' unknown
      exit 4
    }
    printf 'is-active %s %s\n' "$1" "$state" >>"$MAYHEM_MOCK_LOG"
    printf '%s\n' "$state"
    [[ "$state" == "active" ]] || exit 3
    ;;
  stop)
    for unit in "$@"; do
      printf 'stop %s\n' "$unit" >>"$MAYHEM_MOCK_LOG"
      if [[ "$unit" == "${MAYHEM_MOCK_STOP_FAIL_UNIT:-}" ]]; then
        exit 1
      fi
      if [[ "$unit" != "${MAYHEM_MOCK_STICKY_UNIT:-}" ]]; then
        set_state "$unit" inactive
      fi
    done
    ;;
  start)
    for unit in "$@"; do
      printf 'start %s\n' "$unit" >>"$MAYHEM_MOCK_LOG"
      if [[ "$unit" == "${MAYHEM_MOCK_START_FAIL_UNIT:-}" ]]; then
        exit 1
      fi
      set_state "$unit" active
      if [[ "$unit" == "mayhem-payout-worker.timer" &&
            -n "${MAYHEM_MOCK_TIMER_WAKE_UNIT:-}" ]]; then
        set_state "$MAYHEM_MOCK_TIMER_WAKE_UNIT" active
      fi
    done
    ;;
  *)
    printf 'unexpected systemctl invocation: %s %s\n' "$command" "$*" >&2
    exit 1
    ;;
esac
MOCK

chmod +x "$mock_bin"/*

is_initially_active() {
  local candidate="$1" active
  for active in "${ACTIVE_UNITS[@]}"; do
    [[ "$candidate" != "$active" ]] || return 0
  done
  return 1
}

write_initial_state() {
  local state_file="$1" unit state
  : >"$state_file"
  for unit in "${UNITS[@]}"; do
    state=inactive
    if is_initially_active "$unit"; then
      state=active
    fi
    printf '%s %s\n' "$unit" "$state" >>"$state_file"
  done
}

assert_original_state() {
  local state_file="$1" unit expected actual
  for unit in "${UNITS[@]}"; do
    expected=inactive
    if is_initially_active "$unit"; then
      expected=active
    fi
    actual="$(awk -v unit="$unit" '$1 == unit { print $2 }' "$state_file")"
    [[ "$actual" == "$expected" ]] ||
      fail "$unit ended $actual instead of $expected"
  done
}

run_case() {
  local name="$1"
  shift
  CASE_DIR="$tmp/$name"
  CASE_BACKUP_DIR="$CASE_DIR/backups"
  CASE_LOG="$CASE_DIR/events.log"
  CASE_STATE="$CASE_DIR/states"
  CASE_STDOUT="$CASE_DIR/stdout"
  CASE_STDERR="$CASE_DIR/stderr"
  mkdir -p "$CASE_BACKUP_DIR"
  : >"$CASE_LOG"
  write_initial_state "$CASE_STATE"

  if env \
    PATH="$mock_bin:/usr/bin:/bin" \
    MAYHEM_ROOT=/opt/mayhem \
    MAYHEM_BACKUP_DIR="$CASE_BACKUP_DIR" \
    MAYHEM_BACKUP_RETENTION=14 \
    MAYHEM_MOCK_LOG="$CASE_LOG" \
    MAYHEM_MOCK_STATE="$CASE_STATE" \
    MAYHEM_MOCK_STICKY_UNIT= \
    MAYHEM_MOCK_STOP_FAIL_UNIT= \
    MAYHEM_MOCK_START_FAIL_UNIT= \
    MAYHEM_MOCK_TAR_CREATE_FAIL=0 \
    MAYHEM_MOCK_TAR_VERIFY_FAIL=0 \
    MAYHEM_MOCK_TIMER_WAKE_UNIT= \
    "$@" \
    "$BACKUP_SCRIPT" >"$CASE_STDOUT" 2>"$CASE_STDERR"; then
    CASE_STATUS=0
  else
    CASE_STATUS=$?
  fi
}

array_lines() {
  printf '%s\n' "$@"
}

run_case success MAYHEM_MOCK_TIMER_WAKE_UNIT=mayhem-payout-worker.service
[[ "$CASE_STATUS" == "0" ]] ||
  fail "successful backup exited $CASE_STATUS: $(cat "$CASE_STDERR")"
expected_archive="$CASE_BACKUP_DIR/mayhem-mainnet-20260720T120000Z.tar.gz"
[[ "$(cat "$CASE_STDOUT")" == "$expected_archive" ]] ||
  fail "successful backup did not emit only its archive path"
[[ -s "$expected_archive" ]] || fail "verified archive was not committed"
[[ ! -e "$expected_archive.partial" ]] || fail "partial archive was retained"

actual_stops="$(
  awk '$1 == "tar-copy" { copying = 1 } !copying && $1 == "stop" { print $2 }' \
    "$CASE_LOG"
)"
[[ "$actual_stops" == "$(array_lines "${UNITS[@]}")" ]] ||
  fail "stop inventory or stop order is incomplete"
actual_starts="$(awk '$1 == "start" { print $2 }' "$CASE_LOG")"
[[ "$actual_starts" == "$(array_lines "${RESTORE_UNITS[@]}")" ]] ||
  fail "restore started an inactive unit or used the wrong order"

last_stop="$(
  awk '$1 == "tar-copy" { copying = 1 } !copying && $1 == "stop" { line = NR } END { print line + 0 }' \
    "$CASE_LOG"
)"
copy_line="$(awk '$1 == "tar-copy" { print NR; exit }' "$CASE_LOG")"
verify_line="$(awk '$1 == "tar-verify" { print NR; exit }' "$CASE_LOG")"
first_start="$(awk '$1 == "start" { print NR; exit }' "$CASE_LOG")"
(( last_stop < copy_line )) || fail "archive copy began before all units stopped"
(( copy_line < verify_line )) || fail "archive was not verified after it was copied"
(( verify_line < first_start )) || fail "unit restoration began before verification"
grep -Fq -- '-C / opt/mayhem/.mayhem-local' "$CASE_LOG" ||
  fail "canonical backup source path changed"
[[ "$(grep -c '^stop mayhem-payout-worker.service$' "$CASE_LOG")" == "2" ]] ||
  fail "timer-triggered inactive payout worker was not returned to inactive"
assert_original_state "$CASE_STATE"

run_case sticky MAYHEM_MOCK_STICKY_UNIT=mayhem-paygate.service
[[ "$CASE_STATUS" != "0" ]] || fail "incomplete quiescence reported success"
[[ ! -s "$CASE_STDOUT" ]] || fail "incomplete quiescence emitted success output"
! grep -q '^tar-copy ' "$CASE_LOG" ||
  fail "copy began while a tracked unit remained active"
assert_original_state "$CASE_STATE"

run_case create-failure MAYHEM_MOCK_TAR_CREATE_FAIL=1
[[ "$CASE_STATUS" != "0" ]] || fail "archive creation failure reported success"
[[ ! -s "$CASE_STDOUT" ]] || fail "archive creation failure emitted success output"
! grep -q '^tar-verify ' "$CASE_LOG" ||
  fail "failed archive creation reached verification"
[[ ! -e "$CASE_BACKUP_DIR/mayhem-mainnet-20260720T120000Z.tar.gz.partial" ]] ||
  fail "failed archive creation retained a partial"
assert_original_state "$CASE_STATE"

run_case verify-failure MAYHEM_MOCK_TAR_VERIFY_FAIL=1
[[ "$CASE_STATUS" != "0" ]] || fail "archive verification failure reported success"
[[ ! -s "$CASE_STDOUT" ]] || fail "archive verification failure emitted success output"
grep -q '^tar-verify ' "$CASE_LOG" ||
  fail "verification failure did not exercise archive verification"
[[ ! -e "$CASE_BACKUP_DIR/mayhem-mainnet-20260720T120000Z.tar.gz" ]] ||
  fail "unverified archive was committed"
assert_original_state "$CASE_STATE"

run_case restore-failure MAYHEM_MOCK_START_FAIL_UNIT=mayhem-paygate.service
[[ "$CASE_STATUS" != "0" ]] || fail "unit restoration failure reported success"
[[ ! -s "$CASE_STDOUT" ]] || fail "unit restoration failure emitted success output"
grep -q '^tar-verify ' "$CASE_LOG" ||
  fail "restoration failure occurred before archive verification"

printf 'backup-mainnet-quiescence.test: ok\n'
