#!/usr/bin/env bash
set -euo pipefail

# Actual candidate executables + a disposable signed installation. This is not
# a production release/signature, a release-profile build, or a live rail test.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILT_DIR="${MAYHEM_TEST_BINARY_DIR:?set the explicit candidate binary directory}"
EVIDENCE="${MAYHEM_TEST_OUTPUT_DIR:?set a new disposable output directory}"
[[ "$BUILT_DIR" = /* && "$EVIDENCE" = /* && ! -e "$EVIDENCE" ]] || exit 2
[[ "$(uname -s)" == Darwin || "$(uname -s)" == Linux ]] || exit 2
if [[ -r /proc/sys/kernel/apparmor_restrict_unprivileged_userns && "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns)" == 1 ]]; then
  printf 'Use a disposable Linux host without installer AppArmor setup; this check never changes host policy.\n' >&2
  exit 2
fi
mkdir -m 700 "$EVIDENCE"
export MAYHEM_PACKAGE_RELEASE_SOURCE_ONLY=1
source "$ROOT_DIR/scripts/package-release.sh"
VERSION="$(workspace_version)"
TARGET="$(native_host_target)"
BIN_EXT=""
SOURCE_GIT_SHA="$(git -C "$ROOT_DIR" rev-parse HEAD)"
BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
for bin in "${BINS[@]}"; do
  [[ -x "$BUILT_DIR/$bin" ]] || die "missing candidate executable: $bin"
done
# Some existing siblings expose help but no version argument. Their exact
# hashes are bound below; do not invent a CLI flag for those programs.
for bin in mayhem mayhem-proxy-worker; do
  [[ "$("$BUILT_DIR/$bin" --version)" == "$bin $VERSION" ]] || die "candidate version mismatch: $bin"
done
BASE="mayhem-$VERSION-$TARGET"
STAGE="$EVIDENCE/$BASE"
stage_release_binaries "$BUILT_DIR" "$STAGE"
copy_tracked_allowlist "$STAGE" README.md RULES.md
copy_tracked_allowlist "$STAGE/share/mayhem" "${RELEASE_ASSET_SOURCE_ALLOWLIST[@]}"
copy_tracked_allowlist "$STAGE/share/mayhem" "${INTERCOM_SOURCE_ALLOWLIST[@]}"
# Reuse dependencies without any download, install script or source-tree write.
# Only the disposable copy is sealed; the canonical source seal is untouched.
node "$ROOT_DIR/scripts/tests/proxy-installed-runtime-fixture.mjs" prepare "$ROOT_DIR" "$STAGE"
write_intercom_release_metadata "$STAGE/share/mayhem/intercom" "$EVIDENCE/intercom.json"
write_release_manifest "$STAGE" "$EVIDENCE/intercom.json" "$STAGE/manifest.json"
cp "$STAGE/manifest.json" "$EVIDENCE/package-manifest.json"
write_stage_checksums "$STAGE"
node "$ROOT_DIR/scripts/tests/proxy-installed-runtime-fixture.mjs" sign "$ROOT_DIR" "$STAGE"
COPYFILE_DISABLE=1 tar -czf "$EVIDENCE/$BASE.tar.gz" -C "$EVIDENCE" "$BASE"

# Explicit destinations and skip flags: no HOME override, shell-profile write,
# global install, Node/Pear bootstrap or unrelated process/service restart.
bash "$ROOT_DIR/install.sh" \
  --artifact "$EVIDENCE/$BASE.tar.gz" \
  --manifest "$STAGE/manifest.json" \
  --signature "$EVIDENCE/fixture.sig" \
  --release-key "$EVIDENCE/fixture-key.json" \
  --release-key-id proxy-installed-runtime-fixture \
  --source-git-sha "$SOURCE_GIT_SHA" --version "$VERSION" \
  --install-dir "$EVIDENCE/installed/bin" \
  --skip-node --skip-pear --skip-opencode --no-path-update \
  > "$EVIDENCE/install.log" 2>&1
grep -F "verified and activated signed release $VERSION" "$EVIDENCE/install.log" >/dev/null
for bin in "${BINS[@]}"; do
  cmp "$STAGE/bin/$bin" "$EVIDENCE/installed/bin/$bin"
  "$EVIDENCE/installed/bin/$bin" --help >/dev/null
done

export MAYHEM_SETUP_INSTALLED_ROOT="$EVIDENCE/installed"
export MAYHEM_SETUP_CLI_BINARY="$EVIDENCE/installed/bin/mayhem"
export MAYHEM_SETUP_DAEMON_BINARY="$EVIDENCE/installed/bin/mayhemd"
export MAYHEM_SETUP_CLI_RUN_EVIDENCE="$EVIDENCE/run.json"
export MAYHEM_BOOTSTRAP_CLI_EVIDENCE="$EVIDENCE/bootstrap.json"
for name in bootstrap::bootstrap_cli_actual_guided_first_bundle_and_conflict_recovery run::run_cli_real_supervisor_publication_restart; do
  (cd "$ROOT_DIR" && cargo test --locked -p mayhem-proxy --features test-support --test setup "$name" -- --exact --ignored --nocapture) \
    > "$EVIDENCE/${name%%::*}.log" 2>&1
done
node "$ROOT_DIR/scripts/tests/proxy-installed-runtime-fixture.mjs" review "$ROOT_DIR" "$STAGE"
if [[ "${MAYHEM_TEST_KEEP_PACKAGE:-0}" != 1 ]]; then
  # Keep hashes, manifest and small logs; remove only this check's new payloads.
  rm -rf -- "$STAGE" "$EVIDENCE/installed" "$EVIDENCE/$BASE.tar.gz"
fi
printf 'proxy-installed-runtime: authenticated install, guided setup and supervisor restart passed\n'
