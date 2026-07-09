#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${MAYHEM_DIST_DIR:-$ROOT_DIR/dist}"
VERSION="${MAYHEM_VERSION:-}"
TARGET=""
TARGET_SET=0
SKIP_BUILD=0
RELEASE_KEY_ID="${MAYHEM_RELEASE_KEY_ID:-}"
RELEASE_SEED_FILE="${MAYHEM_RELEASE_SEED_FILE:-}"
RELEASE_KEYS_DIR="${MAYHEM_RELEASE_KEYS_DIR:-$ROOT_DIR/release/keys}"
RELEASE_CREATED_AT="${MAYHEM_RELEASE_CREATED_AT:-}"
RELEASE_SIGNER_BIN="${MAYHEM_RELEASE_SIGNER_BIN:-}"

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
  --release-key-id <id> Release signing key id for manifest signature
  --release-seed-file <path>
                         32-byte Ed25519 release signing seed as hex
  --release-keys-dir <dir>
                         Directory for release public key records
  --release-created-at <iso>
                         Created-at timestamp for a newly written release key record
  --release-signer-bin <path>
                         Host-runnable mayhem binary used for manifest signing
  -h, --help            Show this help

Environment:
  MAYHEM_VERSION        Default version
  MAYHEM_DIST_DIR       Default output directory
  MAYHEM_RELEASE_KEY_ID Release signing key id
  MAYHEM_RELEASE_SEED_FILE
                         Release signing seed file
  MAYHEM_RELEASE_KEYS_DIR
                         Release public key directory
  MAYHEM_RELEASE_CREATED_AT
                         Release key created-at timestamp
  MAYHEM_RELEASE_SIGNER_BIN
                         Host-runnable mayhem binary used for signing
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

workspace_version() {
  awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && $1 == "version" {
      value = $3
      gsub(/"/, "", value)
      print value
      exit
    }
  ' "$ROOT_DIR/Cargo.toml"
}

copy_runtime_tree() {
  local src="$1"
  local dest="$2"
  local base parent

  [[ -d "$src" ]] || die "missing runtime asset directory: $src"
  parent="$(dirname "$src")"
  base="$(basename "$src")"
  mkdir -p "$(dirname "$dest")"
  tar \
    --exclude "$base/node_modules" \
    --exclude "$base/.git" \
    --exclude "$base/tests" \
    --exclude "$base/trac/*/node_modules" \
    --exclude "$base/trac/*/.git" \
    --exclude "$base/trac/*/tests" \
    --exclude "$base/trac/*/test" \
    --exclude "$base/trac/*/coverage" \
    --exclude "$base/trac/*/.cache" \
    --exclude "$base/trac/*/logs" \
    --exclude "$base/trac/*/store" \
    -C "$parent" -cf - "$base" | tar -C "$(dirname "$dest")" -xf -
}

stage_runtime_assets() {
  local asset_dir="$1"

  mkdir -p "$asset_dir"
  cp "$ROOT_DIR/RULES.md" "$asset_dir/RULES.md"
  cp -R "$ROOT_DIR/catalog" "$asset_dir/catalog"
  copy_runtime_tree "$ROOT_DIR/intercom" "$asset_dir/intercom"
  copy_runtime_tree "$ROOT_DIR/contracts" "$asset_dir/contracts"
  mkdir -p "$asset_dir/crates/mayhem-cli/src"
  cp "$ROOT_DIR/crates/mayhem-cli/src/"*.mjs "$asset_dir/crates/mayhem-cli/src/"
}

write_stage_checksums() {
  local stage_dir="$1"
  local rel hash

  : > "$stage_dir/SHA256SUMS"
  while IFS= read -r rel; do
    hash="$(sha256_file "$stage_dir/$rel")"
    printf '%s  %s\n' "$hash" "$rel" >> "$stage_dir/SHA256SUMS"
  done < <(cd "$stage_dir" && find . -type f ! -name SHA256SUMS | sed 's#^\./##' | sort)
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
    --release-key-id)
      [[ $# -ge 2 ]] || die "--release-key-id requires a value"
      RELEASE_KEY_ID="$2"
      shift 2
      ;;
    --release-seed-file)
      [[ $# -ge 2 ]] || die "--release-seed-file requires a value"
      RELEASE_SEED_FILE="$2"
      shift 2
      ;;
    --release-keys-dir)
      [[ $# -ge 2 ]] || die "--release-keys-dir requires a value"
      RELEASE_KEYS_DIR="$2"
      shift 2
      ;;
    --release-created-at)
      [[ $# -ge 2 ]] || die "--release-created-at requires a value"
      RELEASE_CREATED_AT="$2"
      shift 2
      ;;
    --release-signer-bin)
      [[ $# -ge 2 ]] || die "--release-signer-bin requires a value"
      RELEASE_SIGNER_BIN="$2"
      shift 2
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
  WORKSPACE_VERSION="$(workspace_version)"
  if [[ -n "$WORKSPACE_VERSION" ]]; then
    VERSION="v$WORKSPACE_VERSION"
  elif git -C "$ROOT_DIR" describe --tags --always --dirty >/dev/null 2>&1; then
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
ASSET_DIR="$STAGE_DIR/share/mayhem"
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

stage_runtime_assets "$ASSET_DIR"

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

write_stage_checksums "$STAGE_DIR"

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

if [[ -n "$RELEASE_SEED_FILE" ]]; then
  [[ -n "$RELEASE_KEY_ID" ]] || die "--release-key-id is required with --release-seed-file"
  if [[ -z "$RELEASE_SIGNER_BIN" ]]; then
    if [[ "$TARGET" != "$(host_target)" ]]; then
      die "set --release-signer-bin to a host-runnable mayhem binary when signing a cross-target release"
    fi
    RELEASE_SIGNER_BIN="$RELEASE_DIR/mayhem$BIN_EXT"
  fi
  [[ -x "$RELEASE_SIGNER_BIN" ]] || die "release signer binary is not executable: $RELEASE_SIGNER_BIN"
  if [[ -z "$RELEASE_CREATED_AT" ]]; then
    RELEASE_CREATED_AT="$BUILT_AT"
  fi
  SIGNATURE="$OUT_DIR/$ARCHIVE_BASENAME.manifest.json.sig"
  log "signing release manifest"
  "$RELEASE_SIGNER_BIN" release-sign \
    --manifest-path "$OUT_DIR/$ARCHIVE_BASENAME.manifest.json" \
    --signature-output "$SIGNATURE" \
    --keys-dir "$RELEASE_KEYS_DIR" \
    --key-id "$RELEASE_KEY_ID" \
    --seed-file "$RELEASE_SEED_FILE" \
    --write-key \
    --created-at "$RELEASE_CREATED_AT" \
    --force
fi

log "wrote $ARCHIVE"
printf 'Archive SHA-256: %s\n' "$ARCHIVE_HASH"
printf 'Checksum sidecar: %s\n' "$ARCHIVE.sha256"
if [[ -n "${SIGNATURE:-}" ]]; then
  printf 'Manifest signature: %s\n' "$SIGNATURE"
fi
