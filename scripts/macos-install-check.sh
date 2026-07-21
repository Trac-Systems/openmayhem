#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${MAYHEM_MACOS_VERSION:-0.2.32}"
DIST_REL="${MAYHEM_MACOS_DIST_REL:-dist/macos-local-install-check}"
DIST_DIR="${MAYHEM_MACOS_DIST_DIR:-$ROOT_DIR/$DIST_REL}"
SKIP_BUILD="${MAYHEM_MACOS_SKIP_BUILD:-0}"
WORK_ROOT=""

BINS=(
  mayhem
  mayhem-gateway
  mayhem-attestation-verifier
  mayhem-pay
  mayhemd
  mayhem-enclave
  mayhem-paygate
)

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/macos-install-check.sh

Build a macOS release archive, then install it into a temporary clean HOME with
install.sh. The check verifies:
  - archive packaging completes for the local macOS target
  - the explicit unsigned compatibility layout accepts its sidecar checksum
  - packaged SHA256SUMS entries are verified
  - Pear is found or bootstrapped through the installer path
  - pinned opencode is installed with checksum verification
  - a copy/paste PATH command is printed while --no-path-update prevents shell
    profile edits
  - all Mayhem binaries are installed and runnable with --help

Environment:
  MAYHEM_MACOS_VERSION       Artifact version string
                             (default: 0.2.32; canonical semver is required)
  MAYHEM_MACOS_DIST_DIR      Dist output directory
                             (default: dist/macos-local-install-check)
  MAYHEM_MACOS_SKIP_BUILD    Use existing target/release binaries when set to 1
  MAYHEM_MACOS_KEEP_TMP      Keep temporary install home when set to 1
USAGE
}

cleanup() {
  if [[ -n "$WORK_ROOT" && "${MAYHEM_MACOS_KEEP_TMP:-0}" != "1" ]]; then
    rm -rf "$WORK_ROOT"
  fi
}
trap cleanup EXIT

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

[[ "$(uname -s)" == "Darwin" ]] || die "macOS install check must run on Darwin"

package_args=(--version "$VERSION" --out-dir "$DIST_DIR")
package_args+=(--unsigned-layout)
if [[ "$SKIP_BUILD" == "1" ]]; then
  package_args+=(--skip-build)
fi

if [[ "$(sysctl -n sysctl.proc_translated 2>/dev/null || printf '0\n')" == "1" ]]; then
  log "Rosetta detected: this run is compatibility coverage, not native Intel release evidence"
fi

mkdir -p "$DIST_DIR"
log "building macOS release archive"
"$ROOT_DIR/scripts/package-release.sh" "${package_args[@]}"

archive="$(
  find "$DIST_DIR" -maxdepth 1 -type f -name "mayhem-$VERSION-*-apple-darwin.tar.gz" \
    | sort \
    | head -n 1
)"
[[ -n "$archive" ]] || die "macOS archive was not created in $DIST_DIR"
[[ -f "$archive.sha256" ]] || die "checksum sidecar missing: $archive.sha256"

WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-macos-install-check.XXXXXX")"
home_dir="$WORK_ROOT/home"
install_dir="$WORK_ROOT/bin"
npm_prefix="$WORK_ROOT/npm"
out="$WORK_ROOT/install.out"
mkdir -p "$home_dir" "$install_dir" "$npm_prefix"

log "installing $(basename "$archive") into temporary HOME"
HOME="$home_dir" \
MAYHEM_NPM_PREFIX="$npm_prefix" \
"$ROOT_DIR/install.sh" \
  --artifact "$archive" \
  --unsigned-layout \
  --install-dir "$install_dir" \
  --force-opencode \
  --no-path-update \
  >"$out" 2>&1

cat "$out"
grep -F "verified archive SHA-256" "$out" >/dev/null
grep -F "packaged file checksum(s)" "$out" >/dev/null
grep -E "(found pinned Pear 2\.0\.4|installing pinned Pear 2\.0\.4)" "$out" >/dev/null
grep -F "installed opencode v1.17.13" "$out" >/dev/null
grep -F "Copy/paste PATH for this shell session:" "$out" >/dev/null
grep -F "export PATH=\"$install_dir:" "$out" >/dev/null
grep -F "skipping shell profile update" "$out" >/dev/null
grep -F "mayhem CLI smoke test passed" "$out" >/dev/null

for bin in "${BINS[@]}"; do
  test -x "$install_dir/$bin"
  "$install_dir/$bin" --help >/dev/null
done

test -x "$install_dir/opencode"
"$install_dir/opencode" --version >/dev/null

if [[ "${MAYHEM_MACOS_KEEP_TMP:-0}" == "1" ]]; then
  log "kept temporary install root at $WORK_ROOT"
fi

log "macOS local artifact install check passed"
