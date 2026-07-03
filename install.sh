#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-$SCRIPT_DIR}"
INSTALL_DIR="${MAYHEM_INSTALL_DIR:-$HOME/.mayhem/bin}"
VERSION="${MAYHEM_VERSION:-latest}"
ARTIFACT="${MAYHEM_ARTIFACT:-}"
ARTIFACT_URL="${MAYHEM_ARTIFACT_URL:-}"
ARTIFACT_SHA256="${MAYHEM_ARTIFACT_SHA256:-}"
RELEASE_BASE_URL="${MAYHEM_RELEASE_BASE_URL:-}"
FROM_SOURCE="${MAYHEM_FROM_SOURCE:-0}"
SKIP_NODE="${MAYHEM_SKIP_NODE:-0}"
SKIP_PEAR="${MAYHEM_SKIP_PEAR:-0}"
NO_PATH_UPDATE="${MAYHEM_NO_PATH_UPDATE:-0}"
ALLOW_UNVERIFIED="${MAYHEM_ALLOW_UNVERIFIED:-0}"
NPM_PREFIX="${MAYHEM_NPM_PREFIX:-$HOME/.mayhem/node}"
OPENCODE_VERSION="${MAYHEM_OPENCODE_VERSION:-1.17.13}"
SKIP_OPENCODE="${MAYHEM_SKIP_OPENCODE:-0}"
FORCE_OPENCODE="${MAYHEM_FORCE_OPENCODE:-0}"

BINS=(
  mayhem
  mayhem-gateway
  mayhem-pay
  mayhemd
  mayhem-enclave
  mayhem-paygate
)

PATH_ENTRIES=()
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-install-root.XXXXXX")"
VERIFIED_PACKAGE_ROOT=""
VERIFIED_PACKAGE_FILES=""

usage() {
  cat <<'USAGE'
Usage: ./install.sh [options]

Install Mayhem binaries, bootstrap Pear when Node/npm are present, and add Mayhem
to PATH. From a source checkout, ./install.sh defaults to --from-source.

Options:
  --from-source             Build and install release binaries from this checkout
  --source-dir <dir>        Source checkout for --from-source
  --artifact <path>         Install a local release archive
  --artifact-url <url>      Download and install a release archive
  --sha256 <hex>            Expected SHA-256 for the release archive
  --release-base-url <url>  Base URL used with --version when no artifact URL is set
  --version <version>       Version for release artifact lookup (default: latest)
  --install-dir <dir>       Binary install directory (default: ~/.mayhem/bin)
  --skip-node               Do not require Node/npm before Pear checks
  --skip-pear               Do not install or warm up Pear
  --skip-opencode           Do not install the pinned opencode binary
  --opencode-version <ver>  Checksum-pinned opencode version (default: 1.17.13)
  --force-opencode          Install pinned opencode even when one is on PATH
  --no-path-update          Do not edit the shell profile
  --allow-unverified        Allow archive installs without a checksum
  -h, --help                Show this help

Environment mirrors the long options with MAYHEM_* names, for example:
  MAYHEM_ARTIFACT_URL, MAYHEM_ARTIFACT_SHA256, MAYHEM_INSTALL_DIR.
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

log() {
  printf '==> %s\n' "$*" >&2
}

cleanup() {
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

make_temp_dir() {
  mktemp -d "$TMP_ROOT/part.XXXXXX"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

detect_target() {
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
    *) die "unsupported host OS for install.sh: $os" ;;
  esac
}

add_path_entry() {
  local entry="$1"
  local existing
  [[ -n "$entry" ]] || return 0

  if [[ "${#PATH_ENTRIES[@]}" -gt 0 ]]; then
    for existing in "${PATH_ENTRIES[@]}"; do
      if [[ "$existing" == "$entry" ]]; then
        return 0
      fi
    done
  fi

  PATH_ENTRIES+=("$entry")
  export PATH="$entry:$PATH"
}

download_file() {
  local url="$1"
  local output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$output" "$url"
  else
    die "curl or wget is required to download release artifacts"
  fi
}

checksum_from_sidecar() {
  local sidecar="$1"
  grep -Eio '[0-9a-f]{64}' "$sidecar" | head -n 1 || true
}

expected_checksum_for() {
  local archive="$1"
  local sidecar

  if [[ -n "$ARTIFACT_SHA256" ]]; then
    printf '%s\n' "$ARTIFACT_SHA256"
    return 0
  fi

  for sidecar in "$archive.sha256" "$(dirname "$archive")/$(basename "$archive").sha256"; do
    if [[ -f "$sidecar" ]]; then
      checksum_from_sidecar "$sidecar"
      return 0
    fi
  done

  return 0
}

verify_archive() {
  local archive="$1"
  local expected actual

  expected="$(expected_checksum_for "$archive" | tr '[:upper:]' '[:lower:]')"
  if [[ -z "$expected" ]]; then
    if [[ "$ALLOW_UNVERIFIED" == "1" ]]; then
      warn "installing unverified archive because --allow-unverified was set"
      return 0
    fi
    die "missing checksum for $archive; pass --sha256 or place a .sha256 sidecar next to it"
  fi

  actual="$(sha256_file "$archive" | tr '[:upper:]' '[:lower:]')"
  if [[ "$actual" != "$expected" ]]; then
    die "checksum mismatch for $archive: expected $expected, got $actual"
  fi
  log "verified archive SHA-256 $actual"
}

archive_name() {
  local target="$1"

  case "$target" in
    *windows*) printf 'mayhem-%s-%s.zip\n' "$VERSION" "$target" ;;
    *) printf 'mayhem-%s-%s.tar.gz\n' "$VERSION" "$target" ;;
  esac
}

detect_opencode_asset() {
  local os arch asset hash
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$arch" in
    x86_64 | amd64) arch="x64" ;;
    arm64 | aarch64) arch="arm64" ;;
    *) die "unsupported host architecture for opencode: $arch" ;;
  esac

  case "$os" in
    Darwin)
      if [[ "$arch" == "x64" ]]; then
        if [[ "$(sysctl -n hw.optional.avx2_0 2>/dev/null || echo 0)" != "1" ]]; then
          asset="opencode-darwin-x64-baseline.zip"
          hash="172ce4efd3adfed678616ccc70592fac24f424f1dc96c23cf1d2ab037d255e69"
        else
          asset="opencode-darwin-x64.zip"
          hash="0bf3d9d134097ca698b83f64c55db960d6d2d0c409069bf4cfd863e5de503b4a"
        fi
      else
        asset="opencode-darwin-arm64.zip"
        hash="dd016d3e26b347d675ab26c45d1e287545912d5c4c49fa0770b622d4a1367e23"
      fi
      ;;
    Linux)
      local musl suffix
      musl=0
      if [[ -f /etc/alpine-release ]]; then
        musl=1
      elif command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
        musl=1
      fi
      suffix=""
      if [[ "$arch" == "x64" ]]; then
        if [[ ! -r /proc/cpuinfo ]] || ! grep -qwi avx2 /proc/cpuinfo; then
          suffix="-baseline"
        fi
      fi
      if [[ "$musl" -eq 1 ]]; then
        suffix="$suffix-musl"
      fi
      asset="opencode-linux-$arch$suffix.tar.gz"
      case "$asset" in
        opencode-linux-arm64.tar.gz) hash="bbaccdd374aaab66cd97c7f8ad1c080aa393610fa5f80ee8dfc007f9500afaf9" ;;
        opencode-linux-arm64-musl.tar.gz) hash="c2323c8c9643ac627a5291d33fba740c029c8487283d5f4f933ef11ac11ee15a" ;;
        opencode-linux-x64.tar.gz) hash="157afa289d1a8d9372de0ce19ac726119b937a1f6b201808d46f06e4e59bb348" ;;
        opencode-linux-x64-baseline.tar.gz) hash="301c245dd81ba80edfb7d6eee7557f58fe0f5174541fb89140b765e554ebc5fd" ;;
        opencode-linux-x64-musl.tar.gz) hash="078ec3e678cc77be11b127660fd3e1f70c676a388ac9cb68cafee8e605b8c2f3" ;;
        opencode-linux-x64-baseline-musl.tar.gz) hash="55da501cfdd88e82294e069b53e68bbea2e130a45ad1d409685478206233f03d" ;;
        *) die "unsupported opencode Linux asset: $asset" ;;
      esac
      ;;
    *)
      die "unsupported host OS for opencode install: $os"
      ;;
  esac

  printf '%s %s\n' "$asset" "$hash"
}

install_opencode() {
  local asset hash archive extract_dir src url actual ext version

  if [[ "$SKIP_OPENCODE" == "1" ]]; then
    log "skipping opencode install"
    return 0
  fi

  if [[ "$FORCE_OPENCODE" != "1" ]] && command -v opencode >/dev/null 2>&1; then
    log "found opencode at $(command -v opencode); skipping pinned install"
    return 0
  fi

  version="${OPENCODE_VERSION#v}"
  [[ "$version" == "1.17.13" ]] || die "opencode installer checksums are pinned for v1.17.13; got v$version"

  read -r asset hash < <(detect_opencode_asset)
  ext="${asset##*.}"
  url="https://github.com/anomalyco/opencode/releases/download/v$version/$asset"
  archive="$(make_temp_dir)/$asset"
  extract_dir="$(make_temp_dir)"

  log "downloading opencode v$version ($asset)"
  download_file "$url" "$archive"
  actual="$(sha256_file "$archive" | tr '[:upper:]' '[:lower:]')"
  if [[ "$actual" != "$hash" ]]; then
    die "opencode checksum mismatch for $asset: expected $hash, got $actual"
  fi

  if [[ "$asset" == *.tar.gz ]]; then
    tar -xzf "$archive" -C "$extract_dir"
  elif [[ "$ext" == "zip" ]]; then
    command -v unzip >/dev/null 2>&1 || die "unzip is required for opencode .zip artifacts"
    unzip -q "$archive" -d "$extract_dir"
  else
    die "unsupported opencode artifact format: $asset"
  fi

  src="$(find "$extract_dir" -type f -name opencode | sort | head -n 1 || true)"
  [[ -n "$src" ]] || die "opencode archive did not contain an opencode binary"
  mkdir -p "$INSTALL_DIR"
  cp "$src" "$INSTALL_DIR/opencode"
  chmod 0755 "$INSTALL_DIR/opencode"
  log "installed opencode v$version into $INSTALL_DIR"
}

download_artifact_if_needed() {
  local target="$1"
  local tmp url_path base archive sidecar_url

  if [[ -n "$ARTIFACT" ]]; then
    printf '%s\n' "$ARTIFACT"
    return 0
  fi

  if [[ -z "$ARTIFACT_URL" ]]; then
    [[ -n "$RELEASE_BASE_URL" ]] || die "set --artifact-url, --artifact, --release-base-url, or use --from-source"
    ARTIFACT_URL="${RELEASE_BASE_URL%/}/$VERSION/$(archive_name "$target")"
  fi

  tmp="$(make_temp_dir)"
  url_path="${ARTIFACT_URL%%\?*}"
  base="$(basename "$url_path")"
  if [[ -z "$base" || "$base" == "/" ]]; then
    base="mayhem-artifact"
  fi
  archive="$tmp/$base"

  log "downloading $ARTIFACT_URL"
  download_file "$ARTIFACT_URL" "$archive"

  if [[ -z "$ARTIFACT_SHA256" ]]; then
    sidecar_url="$ARTIFACT_URL.sha256"
    if download_file "$sidecar_url" "$archive.sha256" >/dev/null 2>&1; then
      log "downloaded checksum sidecar"
    else
      rm -f "$archive.sha256"
    fi
  fi

  printf '%s\n' "$archive"
}

extract_archive() {
  local archive="$1"
  local extract_dir="$2"

  case "$archive" in
    *.tar.gz | *.tgz)
      tar -xzf "$archive" -C "$extract_dir"
      ;;
    *.zip)
      command -v unzip >/dev/null 2>&1 || die "unzip is required for .zip artifacts"
      unzip -q "$archive" -d "$extract_dir"
      ;;
    *)
      die "unsupported artifact format: $archive"
      ;;
  esac
}

verify_extracted_checksums() {
  local extract_dir="$1"
  local sums_file sums_count sums_dir line expected rel target actual verified part
  local -a path_parts

  sums_file="$(find "$extract_dir" -type f -name SHA256SUMS | sort | head -n 1 || true)"
  [[ -n "$sums_file" ]] || die "artifact is missing SHA256SUMS"
  sums_count="$(find "$extract_dir" -type f -name SHA256SUMS | wc -l | tr -d '[:space:]')"
  [[ "$sums_count" == "1" ]] || die "artifact contains multiple SHA256SUMS files"

  sums_dir="$(dirname "$sums_file")"
  verified=0
  VERIFIED_PACKAGE_ROOT="$sums_dir"
  VERIFIED_PACKAGE_FILES=""
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -n "${line//[[:space:]]/}" ]] || continue
    expected="$(printf '%s\n' "$line" | awk '{print $1}' | tr '[:upper:]' '[:lower:]')"
    rel="$(printf '%s\n' "$line" | awk '{$1=""; sub(/^[[:space:]]+/, ""); print}')"

    [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "invalid SHA256SUMS entry: $line"
    [[ -n "$rel" ]] || die "invalid SHA256SUMS entry with empty path"
    [[ "$rel" != *"\\"* ]] || die "unsafe SHA256SUMS path: $rel"
    case "$rel" in
      /* | .. | ../* | */.. | */../*) die "unsafe SHA256SUMS path: $rel" ;;
    esac
    IFS='/' read -r -a path_parts <<< "$rel"
    for part in "${path_parts[@]}"; do
      [[ -n "$part" && "$part" != "." ]] || die "unsafe SHA256SUMS path: $rel"
    done

    target="$sums_dir/$rel"
    [[ -f "$target" ]] || die "SHA256SUMS references missing file: $rel"
    if verified_package_file "$rel"; then
      die "SHA256SUMS contains duplicate path: $rel"
    fi
    actual="$(sha256_file "$target" | tr '[:upper:]' '[:lower:]')"
    [[ "$actual" == "$expected" ]] || die "checksum mismatch for packaged file $rel: expected $expected, got $actual"
    VERIFIED_PACKAGE_FILES+="$rel"$'\n'
    verified=$((verified + 1))
  done < "$sums_file"

  [[ "$verified" -gt 0 ]] || die "SHA256SUMS contains no files"
  log "verified $verified packaged file checksum(s)"
}

verified_package_file() {
  local rel="$1"
  grep -Fx -- "$rel" <<< "$VERIFIED_PACKAGE_FILES" >/dev/null
}

copy_artifact_bins() {
  local package_root="$1"
  local bin rel src

  mkdir -p "$INSTALL_DIR"
  for bin in "${BINS[@]}"; do
    rel="bin/$bin"
    if ! verified_package_file "$rel"; then
      rel="$bin"
      verified_package_file "$rel" || die "SHA256SUMS does not verify binary: $bin"
    fi
    src="$package_root/$rel"
    [[ -f "$src" ]] || die "artifact is missing verified binary: $rel"
    cp "$src" "$INSTALL_DIR/$bin"
    chmod 0755 "$INSTALL_DIR/$bin"
  done
}

install_from_artifact() {
  local target archive extract_dir

  target="$(detect_target)"
  archive="$(download_artifact_if_needed "$target")"
  [[ -f "$archive" ]] || die "artifact not found: $archive"
  verify_archive "$archive"

  extract_dir="$(make_temp_dir)"
  extract_archive "$archive" "$extract_dir"
  verify_extracted_checksums "$extract_dir"
  copy_artifact_bins "$VERIFIED_PACKAGE_ROOT"
}

install_from_source() {
  local bin src

  [[ -f "$SOURCE_DIR/Cargo.toml" ]] || die "source dir does not contain Cargo.toml: $SOURCE_DIR"
  command -v cargo >/dev/null 2>&1 || die "Rust/Cargo is required for --from-source installs"

  log "building release binaries from $SOURCE_DIR"
  (cd "$SOURCE_DIR" && cargo build --release --workspace --bins)

  mkdir -p "$INSTALL_DIR"
  for bin in "${BINS[@]}"; do
    src="$SOURCE_DIR/target/release/$bin"
    [[ -f "$src" ]] || die "missing built binary: $src"
    cp "$src" "$INSTALL_DIR/$bin"
    chmod 0755 "$INSTALL_DIR/$bin"
  done
}

ensure_node() {
  if [[ "$SKIP_NODE" == "1" ]]; then
    return 0
  fi

  if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
    die "Node.js with npm is required for Pear bootstrap; install Node.js 20+ or rerun with --skip-pear"
  fi

  log "found $(node --version) and npm $(npm --version)"
}

ensure_pear() {
  if [[ "$SKIP_PEAR" == "1" ]]; then
    log "skipping Pear bootstrap"
    return 0
  fi

  if command -v pear >/dev/null 2>&1; then
    log "found Pear at $(command -v pear)"
    pear --help >/dev/null 2>&1 || true
    return 0
  fi

  ensure_node
  mkdir -p "$NPM_PREFIX"
  log "installing Pear runtime with npm prefix $NPM_PREFIX"
  npm install -g pear --prefix "$NPM_PREFIX"
  add_path_entry "$NPM_PREFIX/bin"

  if ! command -v pear >/dev/null 2>&1; then
    die "Pear was installed but is not on PATH"
  fi
  pear --help >/dev/null 2>&1 || warn "Pear installed; run 'pear' once if it asks to finish local setup"
}

choose_profile() {
  if [[ -n "${MAYHEM_PROFILE:-}" ]]; then
    printf '%s\n' "$MAYHEM_PROFILE"
    return 0
  fi

  case "${SHELL:-}" in
    */zsh) printf '%s\n' "$HOME/.zshrc" ;;
    */bash) printf '%s\n' "$HOME/.bashrc" ;;
    *) printf '%s\n' "$HOME/.profile" ;;
  esac
}

path_prefix() {
  local joined="" entry
  for entry in "${PATH_ENTRIES[@]}"; do
    if [[ -z "$joined" ]]; then
      joined="$entry"
    else
      joined="$joined:$entry"
    fi
  done
  printf '%s\n' "$joined"
}

update_shell_profile() {
  local profile begin end tmp joined

  [[ "${#PATH_ENTRIES[@]}" -gt 0 ]] || return 0
  joined="$(path_prefix)"

  printf '\nCopy/paste PATH for this shell session:\n'
  printf '  export PATH="%s:$PATH"\n' "$joined"

  if [[ "$NO_PATH_UPDATE" == "1" ]]; then
    log "skipping shell profile update"
    return 0
  fi

  profile="$(choose_profile)"
  begin="# >>> mayhem installer >>>"
  end="# <<< mayhem installer <<<"
  mkdir -p "$(dirname "$profile")"
  touch "$profile"
  tmp="$(make_temp_dir)/profile"

  awk -v begin="$begin" -v end="$end" '
    $0 == begin { skip = 1; next }
    $0 == end { skip = 0; next }
    skip != 1 { print }
  ' "$profile" > "$tmp"

  {
    printf '\n%s\n' "$begin"
    printf 'export PATH="%s:$PATH"\n' "$joined"
    printf '%s\n' "$end"
  } >> "$tmp"

  mv "$tmp" "$profile"
  log "updated PATH block in $profile"
}

smoke_test() {
  if "$INSTALL_DIR/mayhem" --help >/dev/null; then
    log "mayhem CLI smoke test passed"
  else
    die "installed mayhem binary did not run"
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --from-source)
      FROM_SOURCE=1
      shift
      ;;
    --source-dir)
      [[ $# -ge 2 ]] || die "--source-dir requires a value"
      SOURCE_DIR="$2"
      shift 2
      ;;
    --artifact)
      [[ $# -ge 2 ]] || die "--artifact requires a value"
      ARTIFACT="$2"
      shift 2
      ;;
    --artifact-url)
      [[ $# -ge 2 ]] || die "--artifact-url requires a value"
      ARTIFACT_URL="$2"
      shift 2
      ;;
    --sha256)
      [[ $# -ge 2 ]] || die "--sha256 requires a value"
      ARTIFACT_SHA256="$2"
      shift 2
      ;;
    --release-base-url)
      [[ $# -ge 2 ]] || die "--release-base-url requires a value"
      RELEASE_BASE_URL="$2"
      shift 2
      ;;
    --version)
      [[ $# -ge 2 ]] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --install-dir)
      [[ $# -ge 2 ]] || die "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --skip-node)
      SKIP_NODE=1
      shift
      ;;
    --skip-pear)
      SKIP_PEAR=1
      shift
      ;;
    --skip-opencode)
      SKIP_OPENCODE=1
      shift
      ;;
    --opencode-version)
      [[ $# -ge 2 ]] || die "--opencode-version requires a value"
      OPENCODE_VERSION="$2"
      shift 2
      ;;
    --force-opencode)
      FORCE_OPENCODE=1
      shift
      ;;
    --no-path-update)
      NO_PATH_UPDATE=1
      shift
      ;;
    --allow-unverified)
      ALLOW_UNVERIFIED=1
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

if [[ "$FROM_SOURCE" != "1" && -z "$ARTIFACT" && -z "$ARTIFACT_URL" && -z "$RELEASE_BASE_URL" ]]; then
  if [[ -f "$SOURCE_DIR/Cargo.toml" && -d "$SOURCE_DIR/crates/mayhem-cli" ]]; then
    FROM_SOURCE=1
  fi
fi

add_path_entry "$INSTALL_DIR"
ensure_pear

if [[ "$FROM_SOURCE" == "1" ]]; then
  install_from_source
else
  install_from_artifact
fi

install_opencode
update_shell_profile
smoke_test

log "installed Mayhem binaries into $INSTALL_DIR"
