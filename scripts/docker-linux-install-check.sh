#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_IMAGE="${MAYHEM_DOCKER_BUILD_IMAGE:-rust:1.89-bookworm}"
INSTALL_IMAGE="${MAYHEM_DOCKER_INSTALL_IMAGE:-debian:bookworm-slim}"
VERSION="${MAYHEM_DOCKER_VERSION:-docker-linux-check}"
DIST_REL="${MAYHEM_DOCKER_DIST_REL:-dist/docker-linux-install-check}"
DIST_DIR="${MAYHEM_DOCKER_DIST_DIR:-$ROOT_DIR/$DIST_REL}"
CARGO_CACHE="${MAYHEM_DOCKER_CARGO_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/mayhem/docker-cargo}"
BUILD_APT_PACKAGES="${MAYHEM_DOCKER_BUILD_APT_PACKAGES:-clang libclang-dev cmake pkg-config}"
TARGET_SUBDIR="${MAYHEM_DOCKER_TARGET_SUBDIR:-docker-linux-install-check}"
STAGED_ROOT=""

BINS=(
  mayhem
  mayhem-gateway
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
Usage: scripts/docker-linux-install-check.sh

Build a Linux release archive in a Rust Docker container, then install that
archive in a separate clean Debian container using the generated .sha256
sidecar. The check verifies:
  - archive packaging completes
  - install.sh accepts the sidecar checksum without --sha256
  - install.sh prints a copy/paste PATH command
  - all Mayhem binaries are installed and runnable with --help

Environment:
  MAYHEM_DOCKER_BUILD_IMAGE    Rust image (default: rust:1.89-bookworm)
  MAYHEM_DOCKER_INSTALL_IMAGE  Clean install image (default: debian:bookworm-slim)
  MAYHEM_DOCKER_VERSION        Artifact version string (default: docker-linux-check)
  MAYHEM_DOCKER_DIST_DIR       Dist output directory (default: dist/docker-linux-install-check)
  MAYHEM_DOCKER_CARGO_CACHE    Cargo cache mount (default: ~/.cache/mayhem/docker-cargo)
  MAYHEM_DOCKER_BUILD_APT_PACKAGES
                                Build deps installed in the Rust image
                                (default: clang libclang-dev cmake pkg-config)
  MAYHEM_DOCKER_KEEP_STAGE     Keep temporary staged checkout when set to 1
USAGE
}

cleanup() {
  if [[ -n "$STAGED_ROOT" && "${MAYHEM_DOCKER_KEEP_STAGE:-0}" != "1" ]]; then
    rm -rf "$STAGED_ROOT"
  fi
}
trap cleanup EXIT

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v docker >/dev/null 2>&1 || die "docker is required"

WORK_ROOT="$ROOT_DIR"
WORK_DIST_DIR="$DIST_DIR"

if ! docker run --rm -v "$ROOT_DIR:/work:ro" "$INSTALL_IMAGE" sh -c 'test -f /work/install.sh' >/dev/null 2>&1; then
  command -v rsync >/dev/null 2>&1 || die "Docker cannot mount $ROOT_DIR and rsync is unavailable for staging"
  STAGED_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-docker-work.XXXXXX")"
  log "Docker cannot mount $ROOT_DIR; staging checkout at $STAGED_ROOT"
  rsync -a --delete \
    --exclude .git \
    --exclude target \
    --exclude dist \
    --exclude node_modules \
    --exclude stores \
    "$ROOT_DIR/" "$STAGED_ROOT/"
  WORK_ROOT="$STAGED_ROOT"
  WORK_DIST_DIR="$WORK_ROOT/$DIST_REL"
fi

mkdir -p "$WORK_DIST_DIR" "$CARGO_CACHE" "$WORK_ROOT/target/$TARGET_SUBDIR"

uid="$(id -u)"
gid="$(id -g)"

log "building Linux release archive in $BUILD_IMAGE"
docker run --rm \
  -e HOST_UID="$uid" \
  -e HOST_GID="$gid" \
  -e HOME=/tmp/mayhem-home \
  -e CARGO_HOME=/tmp/cargo-home \
  -e CARGO_TARGET_DIR="/work/target/$TARGET_SUBDIR" \
  -e MAYHEM_DIST_DIR="/work/$DIST_REL" \
  -e MAYHEM_VERSION="$VERSION" \
  -e MAYHEM_DOCKER_BUILD_APT_PACKAGES="$BUILD_APT_PACKAGES" \
  -e PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  -v "$WORK_ROOT:/work" \
  -v "$CARGO_CACHE:/tmp/cargo-home" \
  -w /work \
  "$BUILD_IMAGE" \
  bash -c '
    set -euo pipefail
    cleanup_ownership() {
      chown -R "$HOST_UID:$HOST_GID" "$MAYHEM_DIST_DIR" "$CARGO_TARGET_DIR" /tmp/cargo-home 2>/dev/null || true
    }
    trap cleanup_ownership EXIT
    if [[ -n "${MAYHEM_DOCKER_BUILD_APT_PACKAGES:-}" ]]; then
      apt-get update >/dev/null
      apt-get install -y --no-install-recommends $MAYHEM_DOCKER_BUILD_APT_PACKAGES >/dev/null
      rm -rf /var/lib/apt/lists/*
    fi
    scripts/package-release.sh --version "$MAYHEM_VERSION" --out-dir "$MAYHEM_DIST_DIR"
  '

archive="$(
  find "$WORK_DIST_DIR" -maxdepth 1 -type f -name "mayhem-$VERSION-*-unknown-linux-gnu.tar.gz" \
    | sort \
    | head -n 1
)"
[[ -n "$archive" ]] || die "Linux archive was not created in $WORK_DIST_DIR"
[[ -f "$archive.sha256" ]] || die "checksum sidecar missing: $archive.sha256"
archive_base="$(basename "$archive")"

bin_list=""
for bin in "${BINS[@]}"; do
  if [[ -z "$bin_list" ]]; then
    bin_list="$bin"
  else
    bin_list="$bin_list $bin"
  fi
done

log "installing $archive_base in clean $INSTALL_IMAGE"
docker run --rm \
  -e ARCHIVE_BASENAME="$archive_base" \
  -e EXPECTED_BINS="$bin_list" \
  -v "$WORK_ROOT:/work:ro" \
  -v "$WORK_DIST_DIR:/dist:ro" \
  "$INSTALL_IMAGE" \
  bash -c '
    set -euo pipefail
    install_dir=/tmp/mayhem-install/bin
    out=/tmp/mayhem-install.out

    /work/install.sh \
      --artifact "/dist/$ARCHIVE_BASENAME" \
      --install-dir "$install_dir" \
      --skip-pear \
      --skip-opencode \
      --no-path-update \
      >"$out" 2>&1

    cat "$out"
    grep -F "verified archive SHA-256" "$out" >/dev/null
    grep -F "packaged file checksum(s)" "$out" >/dev/null
    grep -F "Copy/paste PATH for this shell session:" "$out" >/dev/null
    grep -F "export PATH=\"$install_dir:" "$out" >/dev/null
    grep -F "mayhem CLI smoke test passed" "$out" >/dev/null

    for bin in $EXPECTED_BINS; do
      test -x "$install_dir/$bin"
      "$install_dir/$bin" --help >/dev/null
    done
  '

log "Linux clean-container artifact install check passed"
