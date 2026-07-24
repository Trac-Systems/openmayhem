#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BACKUP_UNIT="$ROOT_DIR/ops/systemd/mayhem-backup.service"
INSTALLER="$ROOT_DIR/scripts/install-mainnet-systemd.sh"

fail() {
  printf 'backup-systemd-quiescence-ownership.test: %s\n' "$*" >&2
  exit 1
}

[[ -f "$BACKUP_UNIT" ]] || fail "backup service unit is missing"
[[ -f "$INSTALLER" ]] || fail "mainnet systemd installer is missing"

[[ "$(grep -c '^ExecStart=' "$BACKUP_UNIT")" == "1" ]] ||
  fail "backup service must have exactly one ExecStart"
grep -Fqx 'ExecStart=/opt/mayhem/libexec/backup-mainnet.sh' "$BACKUP_UNIT" ||
  fail "backup service does not invoke the root-owned installed helper"
grep -Fqx 'User=root' "$BACKUP_UNIT" ||
  fail "backup service cannot quiesce system units without root authority"
grep -Fqx 'ProtectSystem=strict' "$BACKUP_UNIT" ||
  fail "root backup helper must run with a read-only system"
grep -Fqx 'ReadWritePaths=/opt/mayhem/backups' "$BACKUP_UNIT" ||
  fail "root backup helper must restrict filesystem writes to the backup directory"

if grep -Eq '^Exec(StartPre|StartPost|Stop|StopPost)=' "$BACKUP_UNIT"; then
  fail "backup service contains a competing lifecycle hook"
fi

grep -Fq '"$repo/scripts/ops/backup-mainnet.sh"' "$INSTALLER" ||
  fail "installer does not require the canonical backup source"
grep -Fq 'install -d -m 0750 -o root -g root "$root/libexec"' "$INSTALLER" ||
  fail "installer does not provision the root-owned helper directory"
grep -Fq '"$root/libexec/backup-mainnet.sh"' "$INSTALLER" ||
  fail "installer does not publish the immutable-at-runtime backup helper"
grep -Fq 'canonical units require MAYHEM_ROOT=/opt/mayhem and MAYHEM_REPO=/opt/mayhem/source' "$INSTALLER" ||
  fail "installer does not reject unsupported path overrides"
grep -Fqx 'install -m 0644 "$repo"/ops/systemd/mayhem-*.service /etc/systemd/system/' "$INSTALLER" ||
  fail "installer does not deploy service units unchanged"
grep -Fqx 'systemctl disable --now mayhem-backup.timer' "$INSTALLER" ||
  fail "installer must keep the outage-prone quiesced backup timer disabled"
if sed -n '/^systemctl enable \\/,/^systemctl enable --now mayhem-payout-worker.timer/p' "$INSTALLER" |
  grep -Fq 'mayhem-backup.timer'; then
  fail "installer must not enable the quiesced backup timer"
fi

printf 'backup-systemd-quiescence-ownership.test: ok\n'
