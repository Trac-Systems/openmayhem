#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${MAYHEM_DIST_DIR:-$ROOT_DIR/dist}"
VERSION="${MAYHEM_VERSION:-}"
TARGET=""
TARGET_SET=0
SKIP_BUILD=0

BINS=(
  mayhem
  mayhem-gateway
  mayhem-pay
  mayhemd
  mayhem-enclave
  mayhem-paygate
)

usage() {
  cat <<'USAGE'
Usage: scripts/package-release.sh [options]

Build and package Mayhem release binaries with SHA-256 checksums.

Options:
  --version <version>   Version string for artifact names (default: git describe)
  --target <triple>     Rust target triple to package
  --out-dir <dir>       Output directory (default: dist/)
  --skip-build          Package existing release binaries
  -h, --help            Show this help

Environment:
  MAYHEM_VERSION        Default version
  MAYHEM_DIST_DIR       Default output directory
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

host_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$arch" in
    x86_64 | amd64) arch="x86_64" ;;
    arm64 | aarch64) arch="aarch64" ;;
    *) die "unsupported host architecture: $arch" ;;
  esac

  case "$os" in
    Darwin) printf '%s-apple-darwin\n' "$arch" ;;
    Linux) printf '%s-unknown-linux-gnu\n' "$arch" ;;
    MINGW* | MSYS* | CYGWIN*) printf '%s-pc-windows-msvc\n' "$arch" ;;
    *) die "unsupported host OS: $os" ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --target)
      [[ $# -ge 2 ]] || die "--target requires a value"
      TARGET="$2"
      TARGET_SET=1
      shift 2
      ;;
    --out-dir)
      [[ $# -ge 2 ]] || die "--out-dir requires a value"
      OUT_DIR="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  if git -C "$ROOT_DIR" describe --tags --always --dirty >/dev/null 2>&1; then
    VERSION="$(git -C "$ROOT_DIR" describe --tags --always --dirty)"
  else
    VERSION="dev"
  fi
fi

if [[ -z "$TARGET" ]]; then
  TARGET="$(host_target)"
fi

case "$TARGET" in
  *windows*) BIN_EXT=".exe" ;;
  *) BIN_EXT="" ;;
esac

TARGET_ROOT="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
if [[ "$TARGET_SET" -eq 1 ]]; then
  RELEASE_DIR="$TARGET_ROOT/$TARGET/release"
else
  RELEASE_DIR="$TARGET_ROOT/release"
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  cargo_args=(build --release --workspace --bins)
  if [[ "$TARGET_SET" -eq 1 ]]; then
    cargo_args+=(--target "$TARGET")
  fi
  log "building release binaries"
  (cd "$ROOT_DIR" && cargo "${cargo_args[@]}")
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-package.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

ARCHIVE_BASENAME="mayhem-${VERSION}-${TARGET}"
STAGE_DIR="$TMP_DIR/$ARCHIVE_BASENAME"
mkdir -p "$STAGE_DIR/bin" "$OUT_DIR"

for bin in "${BINS[@]}"; do
  src="$RELEASE_DIR/$bin$BIN_EXT"
  [[ -f "$src" ]] || die "missing built binary: $src"
  cp "$src" "$STAGE_DIR/bin/"
  chmod 0755 "$STAGE_DIR/bin/$bin$BIN_EXT" 2>/dev/null || true
done

for doc in README.md RULES.md; do
  if [[ -f "$ROOT_DIR/$doc" ]]; then
    cp "$ROOT_DIR/$doc" "$STAGE_DIR/"
  fi
done

: > "$STAGE_DIR/SHA256SUMS"
for bin_file in "$STAGE_DIR"/bin/*; do
  hash="$(sha256_file "$bin_file")"
  rel="bin/$(basename "$bin_file")"
  printf '%s  %s\n' "$hash" "$rel" >> "$STAGE_DIR/SHA256SUMS"
done

BUILT_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
MANIFEST="$STAGE_DIR/manifest.json"
{
  printf '{\n'
  printf '  "schema": 1,\n'
  printf '  "name": "mayhem",\n'
  printf '  "version": "%s",\n' "$(json_escape "$VERSION")"
  printf '  "target": "%s",\n' "$(json_escape "$TARGET")"
  printf '  "built_at_utc": "%s",\n' "$(json_escape "$BUILT_AT")"
  printf '  "binaries": [\n'
  for i in "${!BINS[@]}"; do
    bin="${BINS[$i]}$BIN_EXT"
    path="$STAGE_DIR/bin/$bin"
    hash="$(sha256_file "$path")"
    comma=","
    if [[ "$i" -eq $((${#BINS[@]} - 1)) ]]; then
      comma=""
    fi
    printf '    {"name": "%s", "path": "bin/%s", "sha256": "%s"}%s\n' \
      "$(json_escape "$bin")" "$(json_escape "$bin")" "$hash" "$comma"
  done
  printf '  ]\n'
  printf '}\n'
} > "$MANIFEST"

if [[ "$TARGET" == *windows* ]]; then
  command -v zip >/dev/null 2>&1 || die "zip is required for Windows archives"
  ARCHIVE="$OUT_DIR/$ARCHIVE_BASENAME.zip"
  rm -f "$ARCHIVE"
  (cd "$TMP_DIR" && zip -qr "$ARCHIVE" "$ARCHIVE_BASENAME")
else
  ARCHIVE="$OUT_DIR/$ARCHIVE_BASENAME.tar.gz"
  rm -f "$ARCHIVE"
  (cd "$TMP_DIR" && tar -czf "$ARCHIVE" "$ARCHIVE_BASENAME")
fi

ARCHIVE_HASH="$(sha256_file "$ARCHIVE")"
printf '%s  %s\n' "$ARCHIVE_HASH" "$(basename "$ARCHIVE")" > "$ARCHIVE.sha256"
cp "$STAGE_DIR/SHA256SUMS" "$OUT_DIR/$ARCHIVE_BASENAME.SHA256SUMS"
cp "$STAGE_DIR/manifest.json" "$OUT_DIR/$ARCHIVE_BASENAME.manifest.json"

log "wrote $ARCHIVE"
printf 'Archive SHA-256: %s\n' "$ARCHIVE_HASH"
printf 'Checksum sidecar: %s\n' "$ARCHIVE.sha256"
