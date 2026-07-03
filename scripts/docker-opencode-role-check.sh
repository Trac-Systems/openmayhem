#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_IMAGE="${MAYHEM_DOCKER_BUILD_IMAGE:-rust:1.89-bookworm}"
INSTALL_IMAGE="${MAYHEM_DOCKER_INSTALL_IMAGE:-debian:bookworm-slim}"
VERSION="${MAYHEM_DOCKER_VERSION:-docker-opencode-role-check}"
DIST_REL="${MAYHEM_DOCKER_DIST_REL:-dist/docker-opencode-role-check}"
DIST_DIR="${MAYHEM_DOCKER_DIST_DIR:-$ROOT_DIR/$DIST_REL}"
CARGO_CACHE="${MAYHEM_DOCKER_CARGO_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/mayhem/docker-cargo}"
BUILD_APT_PACKAGES="${MAYHEM_DOCKER_BUILD_APT_PACKAGES:-clang libclang-dev cmake pkg-config}"
TARGET_SUBDIR="${MAYHEM_DOCKER_TARGET_SUBDIR:-docker-opencode-role-check}"
STAGED_ROOT=""

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/docker-opencode-role-check.sh

Build a Linux release archive in Docker, install it in a clean Debian container
with the checksum-pinned opencode binary, start the installed gateway, and run:
  - mayhem test --sync-models as role=provider
  - mayhem test --sync-models as role=user

This is local clean-container evidence for P8.2. It does not replace the formal
provider/user reference-machine acceptance gate.

Environment:
  MAYHEM_DOCKER_BUILD_IMAGE    Rust image (default: rust:1.89-bookworm)
  MAYHEM_DOCKER_INSTALL_IMAGE  Clean install image (default: debian:bookworm-slim)
  MAYHEM_DOCKER_VERSION        Artifact version string (default: docker-opencode-role-check)
  MAYHEM_DOCKER_DIST_DIR       Dist output directory (default: dist/docker-opencode-role-check)
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

log "running provider/user opencode role smoke in clean $INSTALL_IMAGE"
docker run --rm \
  -e ARCHIVE_BASENAME="$archive_base" \
  -v "$WORK_ROOT:/work:ro" \
  -v "$WORK_DIST_DIR:/dist:ro" \
  "$INSTALL_IMAGE" \
  bash -c '
    set -euo pipefail

    apt-get update >/dev/null
    apt-get install -y --no-install-recommends ca-certificates curl >/dev/null
    rm -rf /var/lib/apt/lists/*

    install_dir=/tmp/mayhem-install/bin
    /work/install.sh \
      --artifact "/dist/$ARCHIVE_BASENAME" \
      --install-dir "$install_dir" \
      --skip-pear \
      --no-path-update \
      >/tmp/mayhem-install.out 2>&1
    cat /tmp/mayhem-install.out
    grep -F "verified archive SHA-256" /tmp/mayhem-install.out >/dev/null
    grep -F "installed opencode v1.17.13" /tmp/mayhem-install.out >/dev/null
    grep -F "Copy/paste PATH for this shell session:" /tmp/mayhem-install.out >/dev/null

    gateway_log=/tmp/mayhem-gateway.log
    "$install_dir/mayhem-gateway" \
      --dev-embedded-catalog \
      --bind 127.0.0.1:11435 \
      >"$gateway_log" 2>&1 &
    gateway_pid=$!
    cleanup_gateway() {
      kill "$gateway_pid" >/dev/null 2>&1 || true
      wait "$gateway_pid" >/dev/null 2>&1 || true
    }
    trap cleanup_gateway EXIT

    for _ in $(seq 1 60); do
      if curl -fsS http://127.0.0.1:11435/mayhem/status >/dev/null 2>&1; then
        break
      fi
      if ! kill -0 "$gateway_pid" >/dev/null 2>&1; then
        cat "$gateway_log" >&2 || true
        exit 1
      fi
      sleep 1
    done
    curl -fsS http://127.0.0.1:11435/mayhem/status >/dev/null

    run_role() {
      role="$1"
      home="/tmp/mayhem-$role"
      config_home="/tmp/mayhem-$role-config"
      mkdir -p "$home" "$config_home/opencode"
      cat >"$home/config.toml" <<EOF
[role]
mode = "$role"

[network]
gateway_url = "http://127.0.0.1:11435"
rpc_url = "http://127.0.0.1:49223/v1"
EOF

      "$install_dir/mayhem" test \
        --home "$home" \
        --gateway-url http://127.0.0.1:11435 \
        --skip-peer-health \
        --sync-models \
        --opencode-config "$config_home/opencode/opencode.json" \
        --opencode-bin "$install_dir/opencode" \
        --timeout-seconds 90 \
        --json \
        >"/tmp/mayhem-$role-test.json"

      cat "/tmp/mayhem-$role-test.json"
      grep -F "\"ok\": true" "/tmp/mayhem-$role-test.json" >/dev/null
      grep -F "\"role\": \"$role\"" "/tmp/mayhem-$role-test.json" >/dev/null
      grep -F "\"skipped\": true" "/tmp/mayhem-$role-test.json" >/dev/null
      grep -F "\"tool_use_seen\": true" "/tmp/mayhem-$role-test.json" >/dev/null
      grep -F "\"marker_seen\": true" "/tmp/mayhem-$role-test.json" >/dev/null
      grep -F "\"expected_epoch_evidence_key\": \"ev/use/" "/tmp/mayhem-$role-test.json" >/dev/null
      test -s "$config_home/opencode/opencode.json"
      grep -F "\"mayhem\"" "$config_home/opencode/opencode.json" >/dev/null
    }

    run_role provider
    run_role user
  '

log "provider/user opencode role smoke passed"
