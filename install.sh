#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-$SCRIPT_DIR}"
INSTALL_DIR="${MAYHEM_INSTALL_DIR:-$HOME/.mayhem/bin}"
SHARE_DIR="${MAYHEM_SHARE_DIR:-}"
VERSION="${MAYHEM_VERSION:-latest}"
VERSION_EXPLICIT=0
if [[ "${MAYHEM_VERSION+x}" == "x" ]]; then
  VERSION_EXPLICIT=1
fi
ARTIFACT="${MAYHEM_ARTIFACT:-}"
ARTIFACT_URL="${MAYHEM_ARTIFACT_URL:-}"
ARTIFACT_SHA256="${MAYHEM_ARTIFACT_SHA256:-}"
MANIFEST="${MAYHEM_RELEASE_MANIFEST:-}"
MANIFEST_URL="${MAYHEM_RELEASE_MANIFEST_URL:-}"
SIGNATURE="${MAYHEM_RELEASE_SIGNATURE:-}"
SIGNATURE_URL="${MAYHEM_RELEASE_SIGNATURE_URL:-}"
RELEASE_KEY="${MAYHEM_RELEASE_KEY:-}"
RELEASE_KEY_ID="${MAYHEM_RELEASE_KEY_ID:-}"
EXPECTED_SOURCE_GIT_SHA="${MAYHEM_SOURCE_GIT_SHA:-}"
RELEASE_BASE_URL="${MAYHEM_RELEASE_BASE_URL:-}"
FROM_SOURCE="${MAYHEM_FROM_SOURCE:-0}"
UNSIGNED_LAYOUT="${MAYHEM_UNSIGNED_LAYOUT:-0}"
SKIP_NODE="${MAYHEM_SKIP_NODE:-0}"
SKIP_PEAR="${MAYHEM_SKIP_PEAR:-0}"
NO_PATH_UPDATE="${MAYHEM_NO_PATH_UPDATE:-0}"
ALLOW_UNVERIFIED="${MAYHEM_ALLOW_UNVERIFIED:-0}"
NPM_PREFIX="${MAYHEM_NPM_PREFIX:-$HOME/.mayhem/node}"
PEAR_VERSION="2.0.4"
OPENCODE_VERSION="${MAYHEM_OPENCODE_VERSION:-1.17.13}"
SKIP_OPENCODE="${MAYHEM_SKIP_OPENCODE:-0}"
FORCE_OPENCODE="${MAYHEM_FORCE_OPENCODE:-0}"

BINS=(
  mayhem
  mayhem-gateway
  mayhem-attestation-verifier
  mayhem-pay
  mayhemd
  mayhem-enclave
  mayhem-paygate
)

PATH_ENTRIES=()
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-install-root.XXXXXX")"
chmod 0700 "$TMP_ROOT"
SIGNED_ARCHIVE=""
RELEASE_FLOOR_PRESENT=0
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
  --manifest <path>         Detached signed release manifest
  --manifest-url <url>      Download the detached signed release manifest
  --signature <path>        Detached Ed25519 manifest signature
  --signature-url <url>     Download the detached manifest signature
  --release-key <path>      Trusted public release-key record
  --release-key-id <id>     Expected release signing key id
  --source-git-sha <hex>    Expected signed source commit
  --sha256 <hex>            Optional additional archive SHA-256 pin
  --release-base-url <url>  Base URL used with --version when no artifact URL is set
  --version <version>       Exact canonical signed release version
  --install-dir <dir>       Binary install directory (default: ~/.mayhem/bin)
  --share-dir <dir>         Runtime asset directory (default: sibling share/mayhem)
  --skip-node               Do not require Node/npm before Pear checks
  --skip-pear               Do not install or warm up Pear
  --skip-opencode           Do not install the pinned opencode binary
  --opencode-version <ver>  Checksum-pinned opencode version (default: 1.17.13)
  --force-opencode          Install pinned opencode even when one is on PATH
  --no-path-update          Do not edit the shell profile
  --unsigned-layout         Install an updater-ineligible test layout
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

run_as_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    "$@"
    return
  fi
  command -v sudo >/dev/null 2>&1 ||
    die "sudo is required once to install the Ubuntu AppArmor user-namespace profile"
  sudo "$@"
}

apparmor_quote_path() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

configure_linux_user_namespace_sandbox() {
  local restriction profile cli enclave unshare_bin
  local cli_quoted enclave_quoted unshare_quoted smoke_root output

  [[ "$(uname -s)" == "Linux" ]] || return 0
  restriction="/proc/sys/kernel/apparmor_restrict_unprivileged_userns"
  [[ -r "$restriction" ]] || return 0
  [[ "$(cat "$restriction")" == "1" ]] || return 0

  command -v unshare >/dev/null 2>&1 ||
    die "util-linux unshare is required for the Linux enclave sandbox"
  if unshare --user --map-root-user --mount true >/dev/null 2>&1; then
    return 0
  fi
  command -v apparmor_parser >/dev/null 2>&1 ||
    die "AppArmor is restricting user namespaces; install apparmor_parser and rerun install.sh"

  cli="$(readlink -f "$INSTALL_DIR/mayhem")"
  enclave="$(readlink -f "$INSTALL_DIR/mayhem-enclave")"
  unshare_bin="$(readlink -f "$(command -v unshare)")"
  cli_quoted="$(apparmor_quote_path "$cli")"
  enclave_quoted="$(apparmor_quote_path "$enclave")"
  unshare_quoted="$(apparmor_quote_path "$unshare_bin")"
  profile="$TMP_ROOT/mayhem-userns"

  cat > "$profile" <<PROFILE
abi <abi/4.0>,
include <tunables/global>

profile mayhem-userns-cli $cli_quoted flags=(unconfined) {
  userns,
  $unshare_quoted ix,
}

profile mayhem-userns-enclave $enclave_quoted flags=(unconfined) {
  userns,
  $unshare_quoted ix,
}
PROFILE

  log "installing the narrow Ubuntu AppArmor user-namespace profile"
  run_as_root install -m 0644 "$profile" /etc/apparmor.d/mayhem-userns
  run_as_root apparmor_parser -r /etc/apparmor.d/mayhem-userns

  smoke_root="$TMP_ROOT/linux-sandbox-smoke"
  mkdir -p "$smoke_root/read-only" "$smoke_root/writable"
  printf 'mayhem-userns-ready\n' > "$smoke_root/read-only/input.txt"
  if ! output="$(
    "$enclave" sandbox-run \
      --read-only-dir "$smoke_root/read-only" \
      --writable-dir "$smoke_root/writable" \
      -- \
      "$enclave" sandbox-probe-store-read \
      --path "$smoke_root/read-only/input.txt" 2>&1
  )"; then
    die "the installed AppArmor profile did not enable the private Linux sandbox: $output"
  fi
  [[ "$output" == "mayhem-userns-ready" ]] ||
    die "the private Linux sandbox returned an unexpected readiness result: $output"
  log "Linux user-namespace sandbox smoke test passed"
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
    die "unsigned test layout is missing a checksum; pass --sha256 or place a .sha256 sidecar next to it"
  fi

  actual="$(sha256_file "$archive" | tr '[:upper:]' '[:lower:]')"
  if [[ "$actual" != "$expected" ]]; then
    die "checksum mismatch for $archive: expected $expected, got $actual"
  fi
  log "verified archive SHA-256 $actual"
}

archive_name() {
  local target="$1"
  local artifact_version="${VERSION#v}"

  case "$target" in
    *windows*) printf 'mayhem-%s-%s.zip\n' "$artifact_version" "$target" ;;
    *) printf 'mayhem-%s-%s.tar.gz\n' "$artifact_version" "$target" ;;
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

  if [[ "$UNSIGNED_LAYOUT" == "1" && -z "$ARTIFACT_SHA256" ]]; then
    sidecar_url="$ARTIFACT_URL.sha256"
    if download_file "$sidecar_url" "$archive.sha256" >/dev/null 2>&1; then
      log "downloaded checksum sidecar"
    else
      rm -f "$archive.sha256"
    fi
  fi

  printf '%s\n' "$archive"
}

release_artifact_stem() {
  local value="$1"

  case "$value" in
    *.tar.gz) printf '%s\n' "${value%.tar.gz}" ;;
    *.tgz) printf '%s\n' "${value%.tgz}" ;;
    *.zip) printf '%s\n' "${value%.zip}" ;;
    *) die "signed release archive must end in .tar.gz, .tgz, or .zip: $value" ;;
  esac
}

validate_signed_release_selection() {
  if [[ -n "$EXPECTED_SOURCE_GIT_SHA" &&
    ! "$EXPECTED_SOURCE_GIT_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    die "--source-git-sha must be exactly 40 lowercase hexadecimal characters"
  fi

  if [[ "$VERSION" == "latest" ]]; then
    [[ -n "$EXPECTED_SOURCE_GIT_SHA" ]] ||
      die "signed installs cannot use unpinned latest; pass an exact --version or --source-git-sha"
    return 0
  fi

  [[ "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
    die "--version must be an exact canonical semantic version for signed installs"
  [[ "$VERSION_EXPLICIT" == "1" ]] ||
    die "signed installs require an explicitly requested version or source_git_sha"
}

resolve_signed_release_metadata() {
  local archive="$1"
  local tmp local_stem remote_stem

  tmp="$(make_temp_dir)"
  if [[ -z "$MANIFEST" ]]; then
    if [[ -n "$MANIFEST_URL" ]]; then
      MANIFEST="$tmp/manifest.json"
      log "downloading $MANIFEST_URL"
      download_file "$MANIFEST_URL" "$MANIFEST"
    elif [[ -n "$ARTIFACT_URL" ]]; then
      remote_stem="$(release_artifact_stem "${ARTIFACT_URL%%\?*}")"
      MANIFEST_URL="$remote_stem.manifest.json"
      MANIFEST="$tmp/manifest.json"
      log "downloading $MANIFEST_URL"
      download_file "$MANIFEST_URL" "$MANIFEST"
    else
      local_stem="$(release_artifact_stem "$archive")"
      MANIFEST="$local_stem.manifest.json"
    fi
  fi
  if [[ -z "$SIGNATURE" ]]; then
    if [[ -n "$SIGNATURE_URL" ]]; then
      SIGNATURE="$tmp/manifest.json.sig"
      log "downloading $SIGNATURE_URL"
      download_file "$SIGNATURE_URL" "$SIGNATURE"
    elif [[ -n "$ARTIFACT_URL" ]]; then
      remote_stem="$(release_artifact_stem "${ARTIFACT_URL%%\?*}")"
      SIGNATURE_URL="$remote_stem.manifest.json.sig"
      SIGNATURE="$tmp/manifest.json.sig"
      log "downloading $SIGNATURE_URL"
      download_file "$SIGNATURE_URL" "$SIGNATURE"
    else
      local_stem="$(release_artifact_stem "$archive")"
      SIGNATURE="$local_stem.manifest.json.sig"
    fi
  fi
  [[ -n "$RELEASE_KEY" ]] ||
    die "signed installs require --release-key with an independently trusted public key record"
}

snapshot_signed_release_inputs() {
  local archive="$1"
  local snapshot_root

  case "$archive" in
    *.tar.gz) ;;
    *) die "install.sh signed releases require a canonical .tar.gz archive" ;;
  esac

  snapshot_root="$(make_temp_dir)"
  chmod 0700 "$snapshot_root"
  SIGNED_ARCHIVE="$snapshot_root/release.tar.gz"

  RELEASE_SNAPSHOT_ARCHIVE_SOURCE="$archive" \
    RELEASE_SNAPSHOT_MANIFEST_SOURCE="$MANIFEST" \
    RELEASE_SNAPSHOT_SIGNATURE_SOURCE="$SIGNATURE" \
    RELEASE_SNAPSHOT_KEY_SOURCE="$RELEASE_KEY" \
    RELEASE_SNAPSHOT_ARCHIVE="$SIGNED_ARCHIVE" \
    RELEASE_SNAPSHOT_MANIFEST="$snapshot_root/manifest.json" \
    RELEASE_SNAPSHOT_SIGNATURE="$snapshot_root/manifest.json.sig" \
    RELEASE_SNAPSHOT_KEY="$snapshot_root/release-key.json" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const noFollow = fs.constants.O_NOFOLLOW;
if (!Number.isInteger(noFollow) || noFollow === 0) {
  throw new Error('this platform cannot snapshot release inputs without following links');
}
const inputs = [
  {
    source: process.env.RELEASE_SNAPSHOT_ARCHIVE_SOURCE,
    output: process.env.RELEASE_SNAPSHOT_ARCHIVE,
    maximum: 2 * 1024 * 1024 * 1024,
    label: 'release archive',
  },
  {
    source: process.env.RELEASE_SNAPSHOT_MANIFEST_SOURCE,
    output: process.env.RELEASE_SNAPSHOT_MANIFEST,
    maximum: 64 * 1024 * 1024,
    label: 'release manifest',
  },
  {
    source: process.env.RELEASE_SNAPSHOT_SIGNATURE_SOURCE,
    output: process.env.RELEASE_SNAPSHOT_SIGNATURE,
    maximum: 64 * 1024,
    label: 'release signature',
  },
  {
    source: process.env.RELEASE_SNAPSHOT_KEY_SOURCE,
    output: process.env.RELEASE_SNAPSHOT_KEY,
    maximum: 64 * 1024,
    label: 'trusted release key',
  },
].map((input) => {
  if (!input.source) throw new Error(`${input.label} path is required`);
  const source = path.resolve(input.source);
  const before = fs.lstatSync(source);
  if (!before.isFile() || before.isSymbolicLink() ||
      before.size === 0 || before.size > input.maximum) {
    throw new Error(`${input.label} must be a bounded regular non-symlink file`);
  }
  return { ...input, source, before };
});
const identities = new Set();
for (const input of inputs) {
  const identity = `${input.before.dev}:${input.before.ino}`;
  if (identities.has(identity)) {
    throw new Error('release archive, manifest, signature, and key must be distinct files');
  }
  identities.add(identity);
}
for (const input of inputs) {
  let sourceFd;
  let outputFd;
  try {
    sourceFd = fs.openSync(input.source, fs.constants.O_RDONLY | noFollow);
    const opened = fs.fstatSync(sourceFd);
    if (!opened.isFile() ||
        opened.dev !== input.before.dev ||
        opened.ino !== input.before.ino ||
        opened.size !== input.before.size ||
        opened.size === 0 ||
        opened.size > input.maximum) {
      throw new Error(`${input.label} changed while its private snapshot was opened`);
    }
    outputFd = fs.openSync(
      path.resolve(input.output),
      fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_EXCL,
      0o600,
    );
    const buffer = Buffer.allocUnsafe(128 * 1024);
    let total = 0;
    while (true) {
      const read = fs.readSync(sourceFd, buffer, 0, buffer.length, null);
      if (read === 0) break;
      total += read;
      if (total > input.maximum) {
        throw new Error(`${input.label} exceeded its snapshot size limit`);
      }
      let offset = 0;
      while (offset < read) {
        offset += fs.writeSync(outputFd, buffer, offset, read - offset);
      }
    }
    if (total !== opened.size) {
      throw new Error(`${input.label} changed while it was snapshotted`);
    }
    fs.fsyncSync(outputFd);
    fs.chmodSync(input.output, 0o600);
  } finally {
    if (outputFd !== undefined) fs.closeSync(outputFd);
    if (sourceFd !== undefined) fs.closeSync(sourceFd);
  }
}
NODE

  MANIFEST="$snapshot_root/manifest.json"
  SIGNATURE="$snapshot_root/manifest.json.sig"
  RELEASE_KEY="$snapshot_root/release-key.json"
  log "snapshotted signed release inputs into private installer state"
}

validate_signed_install_state() {
  local install_root="$1"
  local update_root="$install_root/.mayhem-update"
  local floor="$update_root/release-floor.json"

  RELEASE_FLOOR_PRESENT=0
  if [[ -e "$install_root" || -L "$install_root" ]]; then
    [[ -d "$install_root" && ! -L "$install_root" ]] ||
      die "release install root must be a real directory: $install_root"
  fi
  if [[ -e "$update_root" || -L "$update_root" ]]; then
    [[ -d "$update_root" && ! -L "$update_root" ]] ||
      die "release update root must be a real directory: $update_root"
  fi
  if [[ -e "$floor" || -L "$floor" ]]; then
    [[ -f "$floor" && ! -L "$floor" ]] ||
      die "release anti-rollback floor must be a regular non-symlink file: $floor"
    RELEASE_FLOOR_PRESENT=1
  fi
}

verify_bootstrap_release_identity() {
  local target="$1"
  local floor="$2"
  local requested_version=""

  if [[ "$VERSION" != "latest" ]]; then
    requested_version="$VERSION"
  fi

  RELEASE_BOOTSTRAP_MANIFEST="$MANIFEST" \
    RELEASE_BOOTSTRAP_SIGNATURE="$SIGNATURE" \
    RELEASE_BOOTSTRAP_KEY="$RELEASE_KEY" \
    RELEASE_BOOTSTRAP_TARGET="$target" \
    RELEASE_BOOTSTRAP_KEY_ID="$RELEASE_KEY_ID" \
    RELEASE_BOOTSTRAP_SOURCE_GIT_SHA="$EXPECTED_SOURCE_GIT_SHA" \
    RELEASE_BOOTSTRAP_REQUESTED_VERSION="$requested_version" \
    RELEASE_BOOTSTRAP_FLOOR="$floor" \
    RELEASE_BOOTSTRAP_FLOOR_PRESENT="$RELEASE_FLOOR_PRESENT" \
    node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const readBounded = (input, maximum, label) => {
  const resolved = path.resolve(input);
  const before = fs.lstatSync(resolved);
  if (!before.isFile() || before.isSymbolicLink() ||
      before.size === 0 || before.size > maximum) {
    throw new Error(`${label} must be a bounded regular non-symlink file`);
  }
  const descriptor = fs.openSync(
    resolved,
    fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW,
  );
  try {
    const opened = fs.fstatSync(descriptor);
    if (!opened.isFile() ||
        opened.dev !== before.dev ||
        opened.ino !== before.ino ||
        opened.size !== before.size ||
        opened.size === 0 ||
        opened.size > maximum) {
      throw new Error(`${label} changed while it was opened`);
    }
    return fs.readFileSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
};
const manifestBytes = readBounded(
  process.env.RELEASE_BOOTSTRAP_MANIFEST,
  64 * 1024 * 1024,
  'release manifest',
);
const signatureBytes = readBounded(
  process.env.RELEASE_BOOTSTRAP_SIGNATURE,
  64 * 1024,
  'release signature',
);
const keyBytes = readBounded(
  process.env.RELEASE_BOOTSTRAP_KEY,
  64 * 1024,
  'trusted release key',
);
const parse = (bytes, label) => {
  try {
    return JSON.parse(bytes.toString('utf8'));
  } catch (error) {
    throw new Error(`${label} is invalid JSON: ${error.message}`);
  }
};
const manifest = parse(manifestBytes, 'release manifest');
const signature = parse(signatureBytes, 'release signature');
const key = parse(keyBytes, 'trusted release key');
const exactKeys = (value, expected) =>
  value !== null &&
  typeof value === 'object' &&
  !Array.isArray(value) &&
  JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...expected].sort());
if (!exactKeys(signature, [
  'schema_version',
  'alg',
  'signed_path',
  'key_id',
  'public_key',
  'sha256',
  'sig',
])) {
  throw new Error('release signature has an unexpected schema');
}
if (!exactKeys(key, ['key_id', 'alg', 'public_key', 'status', 'created_at'])) {
  throw new Error('trusted release key has an unexpected schema');
}
const validKeyId = (value) =>
  typeof value === 'string' &&
  value.length > 0 &&
  value.length <= 128 &&
  /^[A-Za-z0-9._-]+$/.test(value);
if (!validKeyId(key.key_id) ||
    key.alg !== 'ed25519' ||
    key.status !== 'active' ||
    !/^[0-9a-f]{64}$/.test(key.public_key) ||
    signature.schema_version !== 1 ||
    signature.alg !== 'ed25519' ||
    signature.key_id !== key.key_id ||
    signature.public_key !== key.public_key ||
    !/^[0-9a-f]{64}$/.test(signature.sha256) ||
    !/^[0-9a-f]{128}$/.test(signature.sig)) {
  throw new Error('release signature does not match the trusted active Ed25519 key');
}
const expectedKeyId = process.env.RELEASE_BOOTSTRAP_KEY_ID;
if (expectedKeyId && signature.key_id !== expectedKeyId) {
  throw new Error(`release signature key id ${signature.key_id} does not match ${expectedKeyId}`);
}
if (manifest?.schema !== 1 ||
    manifest.name !== 'mayhem' ||
    !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(manifest.version) ||
    manifest.target !== process.env.RELEASE_BOOTSTRAP_TARGET ||
    !/^[0-9a-f]{40}$/.test(manifest.source_git_sha)) {
  throw new Error('signed release identity, target, version, or source_git_sha is invalid');
}
const expectedSource = process.env.RELEASE_BOOTSTRAP_SOURCE_GIT_SHA;
if (expectedSource && !/^[0-9a-f]{40}$/.test(expectedSource)) {
  throw new Error('expected source_git_sha must be exactly 40 lowercase hexadecimal characters');
}
if (expectedSource && manifest.source_git_sha !== expectedSource) {
  throw new Error(
    `signed release source_git_sha ${manifest.source_git_sha} does not match ${expectedSource}`,
  );
}
const expectedSignedPath = `mayhem-${manifest.version}-${manifest.target}.manifest.json`;
if (signature.signed_path !== expectedSignedPath) {
  throw new Error('release signature signed_path does not match the release identity');
}
const digest = crypto.createHash('sha256').update(manifestBytes).digest('hex');
if (signature.sha256 !== digest) {
  throw new Error('release signature manifest hash does not match');
}
const publicKey = crypto.createPublicKey({
  key: Buffer.concat([
    Buffer.from('302a300506032b6570032100', 'hex'),
    Buffer.from(key.public_key, 'hex'),
  ]),
  format: 'der',
  type: 'spki',
});
const signingBytes = Buffer.concat([
  Buffer.from('mayhem.release-manifest.v1\n', 'ascii'),
  manifestBytes,
]);
if (!crypto.verify(null, signingBytes, publicKey, Buffer.from(signature.sig, 'hex'))) {
  throw new Error('release manifest Ed25519 signature verification failed');
}
const requestedVersion = process.env.RELEASE_BOOTSTRAP_REQUESTED_VERSION;
if (requestedVersion &&
    (!/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(requestedVersion) ||
     manifest.version !== requestedVersion)) {
  throw new Error(
    `signed release version ${manifest.version} does not match requested version ${requestedVersion}`,
  );
}
if (!requestedVersion && !expectedSource) {
  throw new Error('signed release is not bound to an exact version or source_git_sha');
}
const floorPath = process.env.RELEASE_BOOTSTRAP_FLOOR;
if (process.env.RELEASE_BOOTSTRAP_FLOOR_PRESENT === '1') {
  const floorBytes = readBounded(floorPath, 4 * 1024, 'release anti-rollback floor');
  const floor = parse(floorBytes, 'release anti-rollback floor');
  if (!exactKeys(floor, ['schema', 'version']) ||
      floor.schema !== 1 ||
      !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(floor.version)) {
    throw new Error('release anti-rollback floor has an invalid schema or version');
  }
  const compareSemver = (left, right) => {
    const leftParts = left.split('.').map(BigInt);
    const rightParts = right.split('.').map(BigInt);
    for (let index = 0; index < 3; index += 1) {
      if (leftParts[index] < rightParts[index]) return -1;
      if (leftParts[index] > rightParts[index]) return 1;
    }
    return 0;
  };
  if (compareSemver(manifest.version, floor.version) < 0) {
    throw new Error(
      `signed release version ${manifest.version} is below protected anti-rollback floor ${floor.version}`,
    );
  }
}
const binaryName = manifest.target.includes('windows') ? 'mayhem.exe' : 'mayhem';
const binaryPath = `bin/${binaryName}`;
const binaries = Array.isArray(manifest.binaries)
  ? manifest.binaries.filter((binary) =>
      binary?.name === binaryName && binary?.path === binaryPath)
  : [];
const assets = Array.isArray(manifest.assets)
  ? manifest.assets.filter((asset) => asset?.path === binaryPath)
  : [];
if (binaries.length !== 1 ||
    assets.length !== 1 ||
    !/^[0-9a-f]{64}$/.test(binaries[0].sha256) ||
    binaries[0].sha256 !== assets[0].sha256) {
  throw new Error('signed manifest does not bind exactly one bootstrap mayhem binary');
}
process.stdout.write([
  manifest.version,
  manifest.source_git_sha,
  signature.key_id,
  binaries[0].sha256,
  binaryPath,
].join('\t') + '\n');
NODE
}

extract_authenticated_bootstrap() {
  local archive="$1"
  local target="$2"
  local version="$3"
  local binary_path="$4"
  local expected_sha="$5"
  local output="$6"
  local archive_entry actual

  archive_entry="mayhem-$version-$target/$binary_path"
  case "$archive" in
    *.tar.gz | *.tgz)
      tar -xOf "$archive" "$archive_entry" >"$output" ||
        die "could not extract the signed bootstrap binary from $archive"
      ;;
    *)
      die "install.sh signed releases require a tar.gz archive for $target"
      ;;
  esac
  actual="$(sha256_file "$output" | tr '[:upper:]' '[:lower:]')"
  [[ "$actual" == "$expected_sha" ]] ||
    die "signed bootstrap binary hash mismatch: expected $expected_sha, got $actual"
  chmod 0755 "$output"
  log "authenticated bootstrap mayhem binary $actual"
}

provision_release_key() {
  local install_root="$1"
  local key_id="$2"
  local update_root="$install_root/.mayhem-update"
  local trusted_root="$update_root/trusted-release-keys"
  local destination="$trusted_root/$key_id.json"

  [[ ! -L "$install_root" ]] || die "release install root must not be a symbolic link"
  if [[ -e "$update_root" || -L "$update_root" ]]; then
    [[ -d "$update_root" && ! -L "$update_root" ]] ||
      die "release update root must be a real directory: $update_root"
  else
    mkdir -p "$update_root"
  fi
  if [[ -e "$trusted_root" || -L "$trusted_root" ]]; then
    [[ -d "$trusted_root" && ! -L "$trusted_root" ]] ||
      die "trusted release key root must be a real directory: $trusted_root"
  else
    mkdir "$trusted_root"
  fi
  if [[ -e "$destination" || -L "$destination" ]]; then
    [[ -f "$destination" && ! -L "$destination" ]] ||
      die "provisioned release key must be a regular non-symlink file: $destination"
    cmp "$RELEASE_KEY" "$destination" >/dev/null ||
      die "provisioned release key $key_id differs from the trusted install state"
  else
    cp "$RELEASE_KEY" "$destination"
    chmod 0644 "$destination"
  fi
  printf '%s\n' "$trusted_root"
}

install_signed_release() {
  local archive="$1"
  local target="$2"
  local identity release_version source_git_sha key_id bootstrap_sha binary_path
  local bootstrap_root bootstrap install_root expected_share trusted_keys stage_report apply_report

  command -v node >/dev/null 2>&1 ||
    die "Node.js is required to authenticate a signed release bootstrap"

  install_root="$(dirname "$INSTALL_DIR")"
  expected_share="$install_root/share/mayhem"
  [[ "$INSTALL_DIR" == "$install_root/bin" ]] ||
    die "signed installs require the binary directory to be <install-root>/bin"
  [[ "$SHARE_DIR" == "$expected_share" ]] ||
    die "signed installs require the asset directory to be <install-root>/share/mayhem"
  validate_signed_install_state "$install_root"

  snapshot_signed_release_inputs "$archive"
  archive="$SIGNED_ARCHIVE"
  identity="$(verify_bootstrap_release_identity \
    "$target" \
    "$install_root/.mayhem-update/release-floor.json")" ||
    die "signed release bootstrap authentication or anti-rollback check failed"
  IFS=$'\t' read -r release_version source_git_sha key_id bootstrap_sha binary_path <<<"$identity"
  [[ -n "$release_version" && -n "$key_id" && -n "$bootstrap_sha" ]] ||
    die "signed release bootstrap identity was incomplete"

  if [[ -n "$ARTIFACT_SHA256" ]]; then
    verify_archive "$archive"
  fi
  bootstrap_root="$(make_temp_dir)"
  bootstrap="$bootstrap_root/mayhem"
  extract_authenticated_bootstrap \
    "$archive" \
    "$target" \
    "$release_version" \
    "$binary_path" \
    "$bootstrap_sha" \
    "$bootstrap"
  [[ "$("$bootstrap" --version)" == "mayhem $release_version" ]] ||
    die "authenticated bootstrap binary version does not match signed release $release_version"

  trusted_keys="$(provision_release_key "$install_root" "$key_id")"

  stage_report="$bootstrap_root/stage.json"
  log "staging and reauthenticating signed release $release_version ($source_git_sha)"
  "$bootstrap" update \
    --home "$install_root" \
    --target "$target" \
    --archive-path "$archive" \
    --manifest-path "$MANIFEST" \
    --signature-path "$SIGNATURE" \
    --release-keys-dir "$trusted_keys" \
    --key-id "$key_id" \
    --json >"$stage_report"

  apply_report="$bootstrap_root/apply.json"
  log "activating signed release $release_version"
  "$bootstrap" update \
    --home "$install_root" \
    --target "$target" \
    --apply-staged \
    --release-keys-dir "$trusted_keys" \
    --key-id "$key_id" \
    --bypass-apply-delay \
    --post-upgrade-arg=--help \
    --json >"$apply_report"
  [[ -f "$install_root/.mayhem-update/release-floor.json" ]] ||
    die "signed install did not provision the anti-rollback floor"
  [[ -f "$trusted_keys/$key_id.json" ]] ||
    die "signed install did not provision updater release trust"
  log "verified and activated signed release $release_version from $source_git_sha"
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

reset_runtime_asset_dir() {
  rm -rf \
    "$SHARE_DIR/RULES.md" \
    "$SHARE_DIR/catalog" \
    "$SHARE_DIR/intercom" \
    "$SHARE_DIR/contracts" \
    "$SHARE_DIR/crates/mayhem-cli/src"
  mkdir -p "$SHARE_DIR"
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

copy_source_assets() {
  reset_runtime_asset_dir
  cp "$SOURCE_DIR/RULES.md" "$SHARE_DIR/RULES.md"
  cp -R "$SOURCE_DIR/catalog" "$SHARE_DIR/catalog"
  copy_runtime_tree "$SOURCE_DIR/intercom" "$SHARE_DIR/intercom"
  copy_runtime_tree "$SOURCE_DIR/contracts" "$SHARE_DIR/contracts"
  mkdir -p "$SHARE_DIR/crates/mayhem-cli/src"
  cp "$SOURCE_DIR/crates/mayhem-cli/src/"*.mjs "$SHARE_DIR/crates/mayhem-cli/src/"
  log "installed Mayhem runtime assets into $SHARE_DIR"
}

copy_artifact_assets() {
  local package_root="$1"
  local rel asset_rel target

  verified_package_file "share/mayhem/RULES.md" || die "SHA256SUMS does not verify share/mayhem/RULES.md"
  reset_runtime_asset_dir
  while IFS= read -r rel || [[ -n "$rel" ]]; do
    case "$rel" in
      share/mayhem/*)
        asset_rel="${rel#share/mayhem/}"
        target="$SHARE_DIR/$asset_rel"
        mkdir -p "$(dirname "$target")"
        cp "$package_root/$rel" "$target"
        ;;
    esac
  done <<< "$VERIFIED_PACKAGE_FILES"
  log "installed Mayhem runtime assets into $SHARE_DIR"
}

hydrate_npm_package() {
  local dir="$1"
  [[ -f "$dir/package.json" ]] || return 0
  log "installing runtime dependencies in $dir"
  if [[ -f "$dir/package-lock.json" ]]; then
    (cd "$dir" && npm ci --omit=dev)
  else
    (cd "$dir" && npm install --omit=dev)
  fi
}

hydrate_intercom_package() {
  local dir="$SHARE_DIR/intercom"
  local verifier="$SOURCE_DIR/scripts/verify-intercom-dependency-topology.mjs"
  local materializer="$dir/scripts/materialize-local-dependencies.mjs"

  [[ -f "$dir/package.json" ]] || die "missing Intercom runtime manifest: $dir/package.json"
  [[ -f "$dir/package-lock.json" ]] || die "missing Intercom root dependency lock: $dir/package-lock.json"
  [[ -f "$dir/.npmrc" ]] || die "missing Intercom root npm configuration: $dir/.npmrc"
  [[ -f "$verifier" ]] || die "missing Intercom dependency topology verifier: $verifier"
  [[ -f "$materializer" ]] || die "missing Intercom local dependency materializer: $materializer"

  log "installing root-authoritative runtime dependencies in $dir"
  (cd "$dir" && npm ci --omit=dev --install-links=true)
  node "$materializer" "$dir"
  node "$verifier" "$dir"
}

hydrate_runtime_assets() {
  if [[ "$SKIP_NODE" == "1" ]]; then
    log "skipping runtime dependency install because --skip-node was set"
    return 0
  fi
  ensure_node
  hydrate_intercom_package
  hydrate_npm_package "$SHARE_DIR/contracts"
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
  if [[ "$UNSIGNED_LAYOUT" != "1" ]]; then
    validate_signed_release_selection
  fi
  archive="$(download_artifact_if_needed "$target")"
  [[ -f "$archive" ]] || die "artifact not found: $archive"
  if [[ "$UNSIGNED_LAYOUT" == "1" ]]; then
    warn "installing an explicit unsigned test layout; updater trust will not be provisioned"
    verify_archive "$archive"
    extract_dir="$(make_temp_dir)"
    extract_archive "$archive" "$extract_dir"
    verify_extracted_checksums "$extract_dir"
    copy_artifact_bins "$VERIFIED_PACKAGE_ROOT"
    copy_artifact_assets "$VERIFIED_PACKAGE_ROOT"
  else
    resolve_signed_release_metadata "$archive"
    install_signed_release "$archive" "$target"
  fi
}

install_from_source() {
  local bin src target_root
  local -a cargo_args

  [[ -f "$SOURCE_DIR/Cargo.toml" ]] || die "source dir does not contain Cargo.toml: $SOURCE_DIR"
  command -v cargo >/dev/null 2>&1 || die "Rust/Cargo is required for --from-source installs"

  log "building release binaries from $SOURCE_DIR"
  cargo_args=(build --release --workspace --bins)
  if [[ "$(uname -s)" == "Darwin" ]]; then
    cargo_args+=(--features mayhem-cli/llama-cpp-metal)
  fi
  (cd "$SOURCE_DIR" && cargo "${cargo_args[@]}")

  target_root="${CARGO_TARGET_DIR:-$SOURCE_DIR/target}"
  if [[ "$target_root" != /* ]]; then
    target_root="$SOURCE_DIR/$target_root"
  fi
  mkdir -p "$INSTALL_DIR"
  for bin in "${BINS[@]}"; do
    src="$target_root/release/$bin"
    [[ -f "$src" ]] || die "missing built binary: $src"
    cp "$src" "$INSTALL_DIR/$bin"
    chmod 0755 "$INSTALL_DIR/$bin"
  done
  copy_source_assets
}

ensure_node() {
  local node_major

  if [[ "$SKIP_NODE" == "1" ]]; then
    return 0
  fi

  if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
    die "Node.js with npm is required for Pear bootstrap; install Node.js 20+ or rerun with --skip-pear"
  fi

  node_major="$(node -p 'Number(process.versions.node.split(".")[0])')"
  if [[ ! "$node_major" =~ ^[0-9]+$ ]] || (( node_major < 20 )); then
    die "Node.js 20+ is required for Pear bootstrap; found $(node --version)"
  fi

  log "found $(node --version) and npm $(npm --version)"
}

ensure_pear() {
  local package_json=""

  if [[ "$SKIP_PEAR" == "1" ]]; then
    log "skipping Pear bootstrap"
    return 0
  fi

  ensure_node
  mkdir -p "$NPM_PREFIX"
  add_path_entry "$NPM_PREFIX/bin"

  for package_json in \
    "$NPM_PREFIX/lib/node_modules/pear/package.json" \
    "$NPM_PREFIX/node_modules/pear/package.json"; do
    if [[ -f "$package_json" && ! -L "$package_json" ]] &&
      node -e '
        const metadata = require(process.argv[1]);
        process.exit(metadata.version === process.argv[2] ? 0 : 1);
      ' "$package_json" "$PEAR_VERSION"; then
      log "found pinned Pear $PEAR_VERSION in $NPM_PREFIX"
      break
    fi
    package_json=""
  done

  if [[ -z "$package_json" ]]; then
    log "installing pinned Pear $PEAR_VERSION with npm prefix $NPM_PREFIX"
    npm install -g "pear@$PEAR_VERSION" --prefix "$NPM_PREFIX"
    for package_json in \
      "$NPM_PREFIX/lib/node_modules/pear/package.json" \
      "$NPM_PREFIX/node_modules/pear/package.json"; do
      if [[ -f "$package_json" && ! -L "$package_json" ]] &&
        node -e '
          const metadata = require(process.argv[1]);
          process.exit(metadata.version === process.argv[2] ? 0 : 1);
        ' "$package_json" "$PEAR_VERSION"; then
        break
      fi
      package_json=""
    done
    [[ -n "$package_json" ]] ||
      die "npm did not install the pinned Pear $PEAR_VERSION package"
  fi

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
    --manifest)
      [[ $# -ge 2 ]] || die "--manifest requires a value"
      MANIFEST="$2"
      shift 2
      ;;
    --manifest-url)
      [[ $# -ge 2 ]] || die "--manifest-url requires a value"
      MANIFEST_URL="$2"
      shift 2
      ;;
    --signature)
      [[ $# -ge 2 ]] || die "--signature requires a value"
      SIGNATURE="$2"
      shift 2
      ;;
    --signature-url)
      [[ $# -ge 2 ]] || die "--signature-url requires a value"
      SIGNATURE_URL="$2"
      shift 2
      ;;
    --release-key)
      [[ $# -ge 2 ]] || die "--release-key requires a value"
      RELEASE_KEY="$2"
      shift 2
      ;;
    --release-key-id)
      [[ $# -ge 2 ]] || die "--release-key-id requires a value"
      RELEASE_KEY_ID="$2"
      shift 2
      ;;
    --source-git-sha)
      [[ $# -ge 2 ]] || die "--source-git-sha requires a value"
      EXPECTED_SOURCE_GIT_SHA="$2"
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
      VERSION_EXPLICIT=1
      shift 2
      ;;
    --install-dir)
      [[ $# -ge 2 ]] || die "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --share-dir)
      [[ $# -ge 2 ]] || die "--share-dir requires a value"
      SHARE_DIR="$2"
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
    --unsigned-layout)
      UNSIGNED_LAYOUT=1
      shift
      ;;
    --allow-unverified)
      die "--allow-unverified has been removed; use --unsigned-layout only for explicit test fixtures"
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

[[ "$ALLOW_UNVERIFIED" != "1" ]] ||
  die "MAYHEM_ALLOW_UNVERIFIED has been removed; unsigned production installs are disabled"
[[ "$UNSIGNED_LAYOUT" == "0" || "$UNSIGNED_LAYOUT" == "1" ]] ||
  die "MAYHEM_UNSIGNED_LAYOUT must be 0 or 1"

if [[ "$FROM_SOURCE" != "1" && -z "$ARTIFACT" && -z "$ARTIFACT_URL" && -z "$RELEASE_BASE_URL" ]]; then
  if [[ -f "$SOURCE_DIR/Cargo.toml" && -d "$SOURCE_DIR/crates/mayhem-cli" ]]; then
    FROM_SOURCE=1
  fi
fi
if [[ "$UNSIGNED_LAYOUT" == "1" && "$FROM_SOURCE" == "1" ]]; then
  die "--unsigned-layout applies only to test release archives"
fi

if [[ -z "$SHARE_DIR" ]]; then
  SHARE_DIR="$(dirname "$INSTALL_DIR")/share/mayhem"
fi

add_path_entry "$INSTALL_DIR"

if [[ "$FROM_SOURCE" == "1" ]]; then
  ensure_pear
  install_from_source
  hydrate_runtime_assets
else
  install_from_artifact
  ensure_pear
fi

configure_linux_user_namespace_sandbox
install_opencode
update_shell_profile
smoke_test

log "installed Mayhem binaries into $INSTALL_DIR"
log "installed Mayhem runtime assets into $SHARE_DIR"
