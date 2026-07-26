#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${MAYHEM_DIST_DIR:-$ROOT_DIR/dist}"
VERSION="${MAYHEM_VERSION:-}"
TARGET=""
TARGET_SET=0
SKIP_BUILD=0
UNSIGNED_LAYOUT=0
VERIFIER_IDENTITY_FILE=""
RELEASE_KEY_ID="${MAYHEM_RELEASE_KEY_ID:-}"
RELEASE_SEED_FILE="${MAYHEM_RELEASE_SEED_FILE:-}"
RELEASE_KEYS_DIR="${MAYHEM_RELEASE_KEYS_DIR:-$ROOT_DIR/release/keys}"
RELEASE_CREATED_AT="${MAYHEM_RELEASE_CREATED_AT:-}"

BINS=(
  mayhem
  mayhem-gateway
  mayhem-attestation-verifier
  mayhem-pay
  mayhemd
  mayhem-enclave
  mayhem-paygate
)

MANAGED_VERIFIER_ID="mayhem-attestation-verifier"
MANAGED_VERIFIER_MANIFEST_SCHEMA_VERSION=1
MANAGED_VERIFIER_IDENTITY_MAX_BYTES=4096
MANAGED_VERIFIER_TARGETS=(
  aarch64-apple-darwin
  aarch64-pc-windows-msvc
  aarch64-unknown-linux-gnu
  x86_64-apple-darwin
  x86_64-pc-windows-msvc
  x86_64-unknown-linux-gnu
)

RELEASE_ASSET_SOURCE_ALLOWLIST=(
  RULES.md
  catalog
  contracts/package.json
  contracts/package-lock.json
  contracts/scripts
  contracts/src
  crates/mayhem-cli/src/msb-transfer-helper.mjs
)

INTERCOM_SOURCE_ALLOWLIST=(
  intercom/.npmrc
  intercom/package.json
  intercom/package-lock.json
  intercom/contract
  intercom/features
  intercom/scripts
  intercom/src
  intercom/trac/msb/package.json
  intercom/trac/msb/package-lock.json
  intercom/trac/msb/msb.mjs
  intercom/trac/msb/migration
  intercom/trac/msb/proto
  intercom/trac/msb/rpc
  intercom/trac/msb/src
  intercom/trac/msb/whitelist
  intercom/trac/trac-peer/package.json
  intercom/trac/trac-peer/package-lock.json
  intercom/trac/trac-peer/rpc
  intercom/trac/trac-peer/scripts/run-peer.mjs
  intercom/trac/trac-peer/src
)

usage() {
  cat <<'USAGE'
Usage: scripts/package-release.sh [options]

Build and package Mayhem release binaries with SHA-256 checksums.

Options:
  --version <version>   Version string for artifact names (default: workspace version)
  --target <triple>     Rust target triple to package
  --out-dir <dir>       Output directory (default: dist/)
  --skip-build          Use existing binaries in an explicit unsigned layout
  --unsigned-layout     Emit an unsigned, updater-ineligible test/layout package
  --verifier-identity-file <path>
                         Explicit verifier identity for a cross-target unsigned layout
  --release-key-id <id> Release signing key id for manifest signature
  --release-seed-file <path>
                         32-byte Ed25519 release signing seed as hex
  --release-keys-dir <dir>
                         Directory for release public key records
  --release-created-at <iso>
                         Expected created_at of the canonical public key record
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
                         Expected canonical release key created-at timestamp
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

clean_source_git_sha() {
  local repository="${1:-$ROOT_DIR}"
  local sha dirty

  sha="$(git -C "$repository" rev-parse --verify HEAD)" ||
    die "release source has no resolvable HEAD commit"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] ||
    die "git rev-parse HEAD did not return an exact lowercase 40-hex commit id"
  dirty="$(
    git -C "$repository" status \
      --porcelain=v1 \
      --untracked-files=all \
      --ignore-submodules=none
  )" || die "could not inspect release source status"
  [[ -z "$dirty" ]] ||
    die "release source tree must be clean, including tracked and untracked files"
  printf '%s\n' "$sha"
}

verify_clean_source_git_sha() {
  local expected="$1"
  local repository="${2:-$ROOT_DIR}"
  local actual

  actual="$(clean_source_git_sha "$repository")"
  [[ "$actual" == "$expected" ]] ||
    die "release source HEAD changed during packaging"
}

validate_release_epoch() {
  local epoch="$1"

  [[ "$epoch" =~ ^(0|[1-9][0-9]*)$ ]] ||
    die "SOURCE_DATE_EPOCH must be canonical non-negative integer seconds"
  RELEASE_TIMESTAMP_EPOCH="$epoch" node --input-type=module <<'NODE'
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
if (!Number.isSafeInteger(epoch) || epoch > 253402300799) {
  throw new Error('release timestamp must fit a whole UTC second through year 9999');
}
const date = new Date(epoch * 1000);
if (Number.isNaN(date.valueOf())) throw new Error('release timestamp is invalid');
NODE
}

resolve_release_epoch() {
  local repository="${1:-$ROOT_DIR}"
  local source_sha="${2:-$SOURCE_GIT_SHA}"
  local epoch

  if [[ -n "${SOURCE_DATE_EPOCH+x}" ]]; then
    epoch="$SOURCE_DATE_EPOCH"
  else
    epoch="$(git -C "$repository" show -s --format=%ct "$source_sha")" ||
      die "could not read clean commit timestamp"
  fi
  validate_release_epoch "$epoch"
  printf '%s\n' "$epoch"
}

release_epoch_iso8601() {
  local epoch="$1"

  validate_release_epoch "$epoch"
  RELEASE_TIMESTAMP_EPOCH="$epoch" node --input-type=module <<'NODE'
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
process.stdout.write(new Date(epoch * 1000).toISOString().replace('.000Z', 'Z'));
NODE
}

validate_release_mode() {
  local signed_release="$1"
  local host native_host

  host="$(host_target)"
  native_host="$(native_host_target)"
  if [[ "$signed_release" == "1" ]]; then
    [[ "$UNSIGNED_LAYOUT" == "0" ]] ||
      die "--unsigned-layout cannot be combined with release signing"
    [[ "$SKIP_BUILD" == "0" ]] ||
      die "signed releases refuse --skip-build and require a fresh build"
    [[ "$TARGET" == "$native_host" ]] ||
      die "signed managed-verifier releases must be built and identified on their native target"
    if [[ "$host" == "x86_64-apple-darwin" && "$native_host" == "aarch64-apple-darwin" ]]; then
      die "Rosetta-translated x86_64 packaging is compatibility evidence, not a native signed Intel release"
    fi
    [[ -z "$VERIFIER_IDENTITY_FILE" ]] ||
      die "signed releases derive verifier identity only from the freshly built staged executable"
  else
    [[ "$UNSIGNED_LAYOUT" == "1" ]] ||
      die "packages without --release-seed-file require explicit --unsigned-layout"
    if [[ "$TARGET" != "$host" && -z "$VERIFIER_IDENTITY_FILE" ]]; then
      die "cross-target unsigned layouts require --verifier-identity-file"
    fi
  fi
}

validate_release_artifact_identity() {
  local version="$1"
  local target="$2"

  [[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
    die "release version must be canonical semantic version without a leading v: $version"
  case "$target" in
    aarch64-apple-darwin | \
      aarch64-pc-windows-msvc | \
      aarch64-unknown-linux-gnu | \
      x86_64-apple-darwin | \
      x86_64-pc-windows-msvc | \
      x86_64-unknown-linux-gnu) ;;
    *) die "unsupported managed verifier release target: $target" ;;
  esac
}

managed_verifier_executable_name() {
  local version="$1"
  local target="$2"
  local extension=""

  validate_release_artifact_identity "$version" "$target"
  [[ "$target" == *-windows-* ]] && extension=".exe"
  printf '%s-%s-%s%s\n' "$MANAGED_VERIFIER_ID" "$version" "$target" "$extension"
}

managed_verifier_manifest_name() {
  local version="$1"
  local target="$2"

  validate_release_artifact_identity "$version" "$target"
  printf '%s-%s-%s.manifest.json\n' "$MANAGED_VERIFIER_ID" "$version" "$target"
}

stage_managed_verifier_artifacts() {
  local stage_dir="$1"
  local identity_file="${2:-}"
  local source="$stage_dir/bin/$MANAGED_VERIFIER_ID$BIN_EXT"
  local executable_name manifest_name executable manifest identity_status=0

  executable_name="$(managed_verifier_executable_name "$VERSION" "$TARGET")"
  manifest_name="$(managed_verifier_manifest_name "$VERSION" "$TARGET")"
  executable="$stage_dir/$executable_name"
  manifest="$stage_dir/$manifest_name"
  [[ -f "$source" && ! -L "$source" ]] ||
    die "managed verifier source must be a regular non-symlink file: $source"
  [[ ! -e "$executable" && ! -L "$executable" ]] ||
    die "managed verifier executable output already exists: $executable"
  [[ ! -e "$manifest" && ! -L "$manifest" ]] ||
    die "managed verifier manifest output already exists: $manifest"

  cp "$source" "$executable"
  chmod 0755 "$executable" 2>/dev/null || true
  if [[ -n "$identity_file" ]]; then
    [[ -f "$identity_file" && ! -L "$identity_file" ]] ||
      die "explicit verifier identity must be a regular non-symlink file: $identity_file"
  else
    [[ "$TARGET" == "$(native_host_target)" ]] ||
      die "cross-target managed verifier layout requires an explicit identity fixture"
    [[ -x "$source" ]] ||
      die "native staged managed verifier is not executable: $source"
  fi
  MANAGED_VERIFIER_EXECUTABLE="$executable" \
    MANAGED_VERIFIER_MANIFEST="$manifest" \
    MANAGED_VERIFIER_IDENTITY="$identity_file" \
    MANAGED_VERIFIER_TARGET="$TARGET" \
    MANAGED_VERIFIER_ID="$MANAGED_VERIFIER_ID" \
    MANAGED_VERIFIER_MANIFEST_SCHEMA_VERSION="$MANAGED_VERIFIER_MANIFEST_SCHEMA_VERSION" \
    MANAGED_VERIFIER_IDENTITY_MAX_BYTES="$MANAGED_VERIFIER_IDENTITY_MAX_BYTES" \
    node --input-type=module <<'NODE' || identity_status=$?
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const executable = path.resolve(process.env.MANAGED_VERIFIER_EXECUTABLE);
const output = path.resolve(process.env.MANAGED_VERIFIER_MANIFEST);
const executableStat = fs.lstatSync(executable);
if (!executableStat.isFile() || executableStat.isSymbolicLink()) {
  throw new Error('managed verifier executable must be a regular non-symlink file');
}
if (fs.existsSync(output)) {
  throw new Error(`managed verifier manifest already exists: ${output}`);
}
const maxIdentityBytes = Number(process.env.MANAGED_VERIFIER_IDENTITY_MAX_BYTES);
if (!Number.isSafeInteger(maxIdentityBytes) || maxIdentityBytes <= 0) {
  throw new Error('managed verifier identity exceeds its strict size bound');
}
const identityInput = process.env.MANAGED_VERIFIER_IDENTITY;
let identityBytes;
if (identityInput) {
  const identityPath = path.resolve(identityInput);
  const identityStat = fs.lstatSync(identityPath);
  if (!identityStat.isFile() ||
      identityStat.isSymbolicLink() ||
      identityStat.size === 0 ||
      identityStat.size > maxIdentityBytes) {
    throw new Error('managed verifier identity fixture is not a bounded regular file');
  }
  identityBytes = fs.readFileSync(identityPath, 'utf8');
} else {
  const executed = spawnSync(executable, ['--identity'], {
    encoding: 'utf8',
    env: {},
    input: '',
    maxBuffer: maxIdentityBytes,
    windowsHide: true,
  });
  if (executed.error ||
      executed.signal !== null ||
      executed.status !== 0 ||
      executed.stderr !== '' ||
      typeof executed.stdout !== 'string' ||
      Buffer.byteLength(executed.stdout) === 0 ||
      Buffer.byteLength(executed.stdout) > maxIdentityBytes) {
    throw new Error('staged managed verifier did not emit a bounded clean identity');
  }
  identityBytes = executed.stdout;
}
let identity;
try {
  identity = JSON.parse(identityBytes);
} catch (error) {
  throw new Error(`managed verifier identity is invalid JSON: ${error.message}`);
}
if (identityBytes !== `${JSON.stringify(identity)}\n`) {
  throw new Error('managed verifier identity must be canonical single-line JSON');
}
const exactKeys = (value, expected) =>
  value !== null &&
  typeof value === 'object' &&
  !Array.isArray(value) &&
  JSON.stringify(Object.keys(value)) === JSON.stringify(expected);
if (!exactKeys(identity, [
  'schema_version',
  'verifier_id',
  'version',
  'profiles',
  'max_input_bytes',
  'public_trust_source',
])) {
  throw new Error('managed verifier identity has an unexpected schema');
}
const expectedProfiles = {
  amd_sev_snp_vcek_v1: [1],
  intel_tdx_dcap_v1: [1],
  nvidia_nras_composite_v1: [1],
  nvidia_nvtrust_offline_composite_v1: [1],
};
if (identity.schema_version !== 1 ||
    identity.verifier_id !== process.env.MANAGED_VERIFIER_ID ||
    identity.version !== 1 ||
    !exactKeys(identity.profiles, Object.keys(expectedProfiles)) ||
    JSON.stringify(identity.profiles) !== JSON.stringify(expectedProfiles) ||
    identity.max_input_bytes !== 8 * 1024 * 1024 ||
    identity.public_trust_source !== 'authenticated_admin_policy_input') {
  throw new Error('managed verifier identity does not match the supported verifier contract');
}
const manifestSchemaVersion = Number(process.env.MANAGED_VERIFIER_MANIFEST_SCHEMA_VERSION);
if (manifestSchemaVersion !== 1) {
  throw new Error('managed verifier manifest schema configuration must be 1');
}
const manifest = {
  schema_version: manifestSchemaVersion,
  target: process.env.MANAGED_VERIFIER_TARGET,
  verifier_id: identity.verifier_id,
  version: identity.version,
  executable_sha256: crypto.createHash('sha256')
    .update(fs.readFileSync(executable))
    .digest('hex'),
  profiles: identity.profiles,
};
fs.writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`, {
  flag: 'wx',
  mode: 0o644,
});
NODE
  [[ "$identity_status" == "0" ]] ||
    die "managed verifier identity validation failed"
}

stage_release_binaries() {
  local release_dir="$1"
  local stage_dir="$2"
  local bin source destination

  [[ -d "$release_dir" && ! -L "$release_dir" ]] ||
    die "built binary source directory must be a real directory: $release_dir"
  mkdir -p "$stage_dir/bin"
  for bin in "${BINS[@]}"; do
    source="$release_dir/$bin$BIN_EXT"
    destination="$stage_dir/bin/$bin$BIN_EXT"
    [[ -f "$source" && ! -L "$source" ]] ||
      die "built binary source must be a regular non-symlink file: $source"
    cp "$source" "$destination"
    chmod 0755 "$destination" 2>/dev/null || true
  done
}

prepare_fresh_signed_binary_outputs() {
  local release_dir="$1"
  local bin output

  mkdir -p "$release_dir"
  [[ -d "$release_dir" && ! -L "$release_dir" ]] ||
    die "signed build output directory must be a real directory: $release_dir"
  for bin in "${BINS[@]}"; do
    output="$release_dir/$bin$BIN_EXT"
    [[ ! -L "$output" ]] ||
      die "signed build refuses symlink binary output before rebuilding: $output"
    [[ ! -e "$output" || -f "$output" ]] ||
      die "signed build output path is not a regular file: $output"
    rm -f "$output"
  done
}

normalize_release_stage() {
  local stage_dir="$1"
  local epoch="$2"
  local executable_name bin_names

  executable_name="$(managed_verifier_executable_name "$VERSION" "$TARGET")"
  bin_names="$(printf '%s\n' "${BINS[@]}")"
  RELEASE_STAGE_ROOT="$stage_dir" \
    RELEASE_TIMESTAMP_EPOCH="$epoch" \
    RELEASE_BIN_NAMES="$bin_names" \
    RELEASE_BIN_EXT="$BIN_EXT" \
    MANAGED_EXECUTABLE_NAME="$executable_name" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_STAGE_ROOT);
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
if (!Number.isSafeInteger(epoch) || epoch < 0) {
  throw new Error('release normalization epoch is invalid');
}
const executablePaths = new Set(
  process.env.RELEASE_BIN_NAMES.split('\n').filter(Boolean)
    .map((name) => `bin/${name}${process.env.RELEASE_BIN_EXT}`)
);
executablePaths.add(process.env.MANAGED_EXECUTABLE_NAME);
const directories = [];
const files = [];
const visit = (directory, parent = []) => {
  directories.push({ path: directory, relative: parent.join('/') });
  const entries = fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
  for (const entry of entries) {
    const parts = [...parent, entry.name];
    const relative = parts.join('/');
    const entryPath = path.join(directory, entry.name);
    const stat = fs.lstatSync(entryPath);
    if (stat.isSymbolicLink()) {
      throw new Error(`release normalization rejects symbolic link: ${relative}`);
    }
    if (stat.isDirectory()) {
      visit(entryPath, parts);
    } else if (stat.isFile()) {
      files.push({ path: entryPath, relative });
    } else {
      throw new Error(`release normalization rejects special file: ${relative}`);
    }
  }
};
visit(root);
for (const file of files) {
  fs.chmodSync(file.path, executablePaths.has(file.relative) ? 0o755 : 0o644);
  fs.utimesSync(file.path, epoch, epoch);
}
for (const directory of directories.reverse()) {
  fs.chmodSync(directory.path, 0o755);
  fs.utimesSync(directory.path, epoch, epoch);
}
NODE
}

write_release_archive_file_list() {
  local stage_dir="$1"
  local output="$2"

  RELEASE_STAGE_ROOT="$stage_dir" \
    RELEASE_ARCHIVE_LIST="$output" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_STAGE_ROOT);
const output = path.resolve(process.env.RELEASE_ARCHIVE_LIST);
const archiveRoot = path.basename(root);
if (!/^[A-Za-z0-9._-]+$/.test(archiveRoot)) {
  throw new Error(`release archive root is unsafe: ${archiveRoot}`);
}
const archivedPaths = [`${archiveRoot}/`];
const safe = (relative) => {
  if (!/^[\x20-\x7e]+$/.test(relative) ||
      relative.includes('\n') ||
      relative.includes('\r') ||
      relative.startsWith('/') ||
      relative.includes('\\') ||
      /[<>:"|?*]/.test(relative) ||
      relative.split('/').some((part) =>
        part.length === 0 || part === '.' || part === '..' ||
        part.endsWith('.') || part.endsWith(' '))) {
    throw new Error(`unsafe release archive path: ${JSON.stringify(relative)}`);
  }
};
const visit = (directory, parent = []) => {
  const entries = fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
  for (const entry of entries) {
    const parts = [...parent, entry.name];
    const relative = parts.join('/');
    safe(relative);
    const entryPath = path.join(directory, entry.name);
    const stat = fs.lstatSync(entryPath);
    if (stat.isSymbolicLink()) throw new Error(`release archive rejects symlink: ${relative}`);
    if (stat.isDirectory()) {
      archivedPaths.push(`${archiveRoot}/${relative}/`);
      visit(entryPath, parts);
    } else if (stat.isFile()) {
      archivedPaths.push(`${archiveRoot}/${relative}`);
    } else {
      throw new Error(`release archive rejects special file: ${relative}`);
    }
  }
};
visit(root);
archivedPaths.sort();
if (archivedPaths.length === 1) throw new Error('release archive cannot be empty');
fs.writeFileSync(output, `${archivedPaths.join('\n')}\n`, { flag: 'wx', mode: 0o644 });
NODE
}

create_deterministic_release_archive() {
  local stage_dir="$1"
  local work_dir="$2"
  local output_dir="$3"
  local archive_basename="$4"
  local target="$5"
  local epoch="$6"
  local file_list="$work_dir/release-archive-files.txt"
  local actual_list="$work_dir/release-archive-actual.txt"
  local tar_file="$work_dir/$archive_basename.tar"
  local tar_version

  rm -f "$file_list" "$actual_list" "$tar_file"
  write_release_archive_file_list "$stage_dir" "$file_list"
  mkdir -p "$output_dir"
  [[ -d "$output_dir" && ! -L "$output_dir" ]] ||
    die "release output directory must be a real directory: $output_dir"
  if [[ "$target" == *-windows-* ]]; then
    [[ "$epoch" -ge 315532800 && "$epoch" -le 4354819198 ]] ||
      die "Windows ZIP timestamps require an epoch from 1980 through 2107"
    ARCHIVE="$output_dir/$archive_basename.zip"
    rm -f "$ARCHIVE"
    create_deterministic_windows_zip \
      "$work_dir" \
      "$file_list" \
      "$actual_list" \
      "$ARCHIVE" \
      "$epoch"
  else
    command -v tar >/dev/null 2>&1 ||
      die "tar is required for deterministic non-Windows archives"
    command -v gzip >/dev/null 2>&1 ||
      die "gzip is required for deterministic non-Windows archives"
    gzip -n -9 -c </dev/null >/dev/null 2>&1 ||
      die "gzip does not support deterministic -n output"
    tar_version="$(tar --version 2>&1 | head -n 1)"
    case "$tar_version" in
      *"GNU tar"*)
        (
          cd "$work_dir"
          LC_ALL=C TZ=UTC tar \
            --format=gnu \
            --owner=0 \
            --group=0 \
            --numeric-owner \
            --mtime="@$epoch" \
            --no-acls \
            --no-xattrs \
            --no-selinux \
            --no-recursion \
            -cf "$tar_file" \
            -T "$file_list"
        )
        ;;
      bsdtar*)
        (
          cd "$work_dir"
          COPYFILE_DISABLE=1 LC_ALL=C TZ=UTC tar \
            --uid 0 \
            --gid 0 \
            --numeric-owner \
            --no-acls \
            --no-xattrs \
            --no-fflags \
            --no-mac-metadata \
            --no-recursion \
            -cf "$tar_file" \
            -T "$file_list"
        )
        ;;
      *) die "unsupported tar implementation for deterministic releases: $tar_version" ;;
    esac
    ARCHIVE="$output_dir/$archive_basename.tar.gz"
    rm -f "$ARCHIVE"
    LC_ALL=C TZ=UTC gzip -n -9 -c "$tar_file" >"$ARCHIVE"
    LC_ALL=C tar -tzf "$ARCHIVE" >"$actual_list"
  fi
  cmp "$file_list" "$actual_list" >/dev/null ||
    die "release archive entry order or inventory is not canonical"
}

create_deterministic_windows_zip() {
  local work_dir="$1"
  local file_list="$2"
  local actual_list="$3"
  local output="$4"
  local epoch="$5"
  local first="$output.first.zip"
  local helper_dir="$work_dir/.mayhem-deterministic-zip"
  local helper_source="$helper_dir/MayhemDeterministicZip.cs"
  local helper_project="$helper_dir/MayhemDeterministicZip.csproj"
  local helper_runner="$helper_dir/run.ps1"
  local powershell_bin=""
  local dotnet_major=""

  rm -f "$output" "$first"
  [[ "$epoch" =~ ^[0-9]+$ ]] ||
    die "Windows ZIP timestamp epoch must be an integer"
  mkdir -p "$helper_dir"
  [[ -d "$helper_dir" && ! -L "$helper_dir" ]] ||
    die "Windows ZIP helper directory must be a real directory"
  if [[ ! -f "$helper_source" || ! -f "$helper_project" || ! -f "$helper_runner" ]]; then
    rm -f "$helper_source" "$helper_project" "$helper_runner"
    WINDOWS_ZIP_HELPER_SOURCE="$helper_source" \
      WINDOWS_ZIP_HELPER_PROJECT="$helper_project" \
      WINDOWS_ZIP_HELPER_RUNNER="$helper_runner" \
      node --input-type=module <<'NODE'
import fs from 'node:fs';

const source = String.raw`using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Text;

public static class MayhemDeterministicZip
{
    private const int MaxEntries = 250000;
    private const int MaxPathBytes = 1024;
    private const long MaxFileBytes = 2L * 1024L * 1024L * 1024L;
    private const long MaxTotalFileBytes = 8L * 1024L * 1024L * 1024L;
    private static readonly UTF8Encoding StrictUtf8 = new UTF8Encoding(false, true);

    public static int Main(string[] args)
    {
        if (args.Length != 5)
        {
            Console.Error.WriteLine(
                "usage: MayhemDeterministicZip WORK LIST OUTPUT EPOCH ACTUAL");
            return 2;
        }
        Create(
            args[0],
            args[1],
            args[2],
            Int64.Parse(args[3], CultureInfo.InvariantCulture),
            args[4]);
        return 0;
    }

    public static void Create(
        string workDirectory,
        string listPath,
        string outputPath,
        long epoch,
        string actualListPath)
    {
        string workRoot = Path.GetFullPath(workDirectory);
        string workPrefix = workRoot.TrimEnd(
            Path.DirectorySeparatorChar,
            Path.AltDirectorySeparatorChar) + Path.DirectorySeparatorChar;
        string[] paths = File.ReadAllLines(listPath, StrictUtf8);
        if (paths.Length < 2 || paths.Length > MaxEntries)
        {
            throw new InvalidDataException("release ZIP entry count is out of bounds");
        }

        var exact = new HashSet<string>(StringComparer.Ordinal);
        var portable = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long totalFileBytes = 0;
        DateTimeOffset timestamp = new DateTimeOffset(
            1970, 1, 1, 0, 0, 0, TimeSpan.Zero).AddSeconds(epoch);

        using (var stream = new FileStream(
            outputPath,
            FileMode.CreateNew,
            FileAccess.ReadWrite,
            FileShare.None))
        using (var archive = new ZipArchive(
            stream,
            ZipArchiveMode.Create,
            false,
            Encoding.UTF8))
        {
            foreach (string archivePath in paths)
            {
                bool isDirectory = ValidateArchivePath(archivePath);
                if (!exact.Add(archivePath) || !portable.Add(archivePath))
                {
                    throw new InvalidDataException(
                        "release ZIP contains a duplicate or case-colliding path");
                }

                string relative = isDirectory
                    ? archivePath.Substring(0, archivePath.Length - 1)
                    : archivePath;
                string nativeRelative = relative.Replace(
                    '/',
                    Path.DirectorySeparatorChar);
                string sourcePath = Path.GetFullPath(
                    Path.Combine(workRoot, nativeRelative));
                if (!sourcePath.StartsWith(
                    workPrefix,
                    StringComparison.OrdinalIgnoreCase))
                {
                    throw new InvalidDataException(
                        "release ZIP source path escapes its work directory");
                }

                FileAttributes attributes = File.GetAttributes(sourcePath);
                if ((attributes & FileAttributes.ReparsePoint) != 0)
                {
                    throw new InvalidDataException(
                        "release ZIP source contains a reparse point");
                }
                bool sourceIsDirectory =
                    (attributes & FileAttributes.Directory) != 0;
                if (sourceIsDirectory != isDirectory)
                {
                    throw new InvalidDataException(
                        "release ZIP source type conflicts with its path");
                }

                ZipArchiveEntry entry = archive.CreateEntry(
                    archivePath,
                    CompressionLevel.NoCompression);
                entry.LastWriteTime = timestamp;
                entry.ExternalAttributes = isDirectory
                    ? unchecked((int)((0x41EDu << 16) | 0x10u))
                    : unchecked((int)(0x81A4u << 16));
                if (isDirectory)
                {
                    using (Stream ignored = entry.Open())
                    {
                    }
                    continue;
                }

                var sourceInfo = new FileInfo(sourcePath);
                if (sourceInfo.Length > MaxFileBytes)
                {
                    throw new InvalidDataException(
                        "release ZIP source file exceeds max_file_bytes");
                }
                totalFileBytes = checked(totalFileBytes + sourceInfo.Length);
                if (totalFileBytes > MaxTotalFileBytes)
                {
                    throw new InvalidDataException(
                        "release ZIP exceeds max_total_file_bytes");
                }
                using (var input = new FileStream(
                    sourcePath,
                    FileMode.Open,
                    FileAccess.Read,
                    FileShare.Read))
                using (Stream destination = entry.Open())
                {
                    input.CopyTo(destination);
                }
            }
        }

        var actual = new List<string>();
        using (var stream = new FileStream(
            outputPath,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read))
        using (var archive = new ZipArchive(
            stream,
            ZipArchiveMode.Read,
            false,
            Encoding.UTF8))
        {
            foreach (ZipArchiveEntry entry in archive.Entries)
            {
                actual.Add(entry.FullName);
            }
        }
        File.WriteAllText(
            actualListPath,
            String.Join("\n", actual.ToArray()) + "\n",
            StrictUtf8);
    }

    private static bool ValidateArchivePath(string archivePath)
    {
        if (String.IsNullOrEmpty(archivePath) ||
            Encoding.UTF8.GetByteCount(archivePath) > MaxPathBytes ||
            archivePath[0] == '/' ||
            archivePath.IndexOf('\\') >= 0)
        {
            throw new InvalidDataException("release ZIP path is unsafe");
        }
        foreach (char value in archivePath)
        {
            if (value < 0x20 || value > 0x7e ||
                "<>:\"|?*".IndexOf(value) >= 0)
            {
                throw new InvalidDataException("release ZIP path is non-portable");
            }
        }

        bool isDirectory = archivePath.EndsWith(
            "/",
            StringComparison.Ordinal);
        string normalized = isDirectory
            ? archivePath.Substring(0, archivePath.Length - 1)
            : archivePath;
        string[] parts = normalized.Split('/');
        if (parts.Length == 0)
        {
            throw new InvalidDataException("release ZIP path is empty");
        }
        foreach (string part in parts)
        {
            if (part.Length == 0 ||
                part == "." ||
                part == ".." ||
                part.EndsWith(".", StringComparison.Ordinal) ||
                part.EndsWith(" ", StringComparison.Ordinal))
            {
                throw new InvalidDataException("release ZIP path is non-portable");
            }
        }
        return isDirectory;
    }
}
`;

const project = `<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net6.0</TargetFramework>
    <ImplicitUsings>disable</ImplicitUsings>
    <Nullable>disable</Nullable>
    <Deterministic>true</Deterministic>
  </PropertyGroup>
</Project>
`;

const runner = `param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$WorkDirectory,
    [Parameter(Mandatory = $true)][string]$ListPath,
    [Parameter(Mandatory = $true)][string]$OutputPath,
    [Parameter(Mandatory = $true)][Int64]$Epoch,
    [Parameter(Mandatory = $true)][string]$ActualListPath
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -Path $Source
[MayhemDeterministicZip]::Create(
    $WorkDirectory,
    $ListPath,
    $OutputPath,
    $Epoch,
    $ActualListPath
)
`;

for (const [file, contents] of [
  [process.env.WINDOWS_ZIP_HELPER_SOURCE, source],
  [process.env.WINDOWS_ZIP_HELPER_PROJECT, project],
  [process.env.WINDOWS_ZIP_HELPER_RUNNER, runner],
]) {
  fs.writeFileSync(file, contents, { flag: 'wx', mode: 0o600 });
}
NODE
  fi

  powershell_bin="$(
    command -v pwsh.exe 2>/dev/null ||
      command -v powershell.exe 2>/dev/null ||
      command -v pwsh 2>/dev/null ||
      command -v powershell 2>/dev/null ||
      true
  )"
  if [[ -n "$powershell_bin" ]]; then
    "$powershell_bin" \
      -NoLogo \
      -NoProfile \
      -NonInteractive \
      -File "$helper_runner" \
      -Source "$helper_source" \
      -WorkDirectory "$work_dir" \
      -ListPath "$file_list" \
      -OutputPath "$output" \
      -Epoch "$epoch" \
      -ActualListPath "$actual_list"
    "$powershell_bin" \
      -NoLogo \
      -NoProfile \
      -NonInteractive \
      -File "$helper_runner" \
      -Source "$helper_source" \
      -WorkDirectory "$work_dir" \
      -ListPath "$file_list" \
      -OutputPath "$first" \
      -Epoch "$epoch" \
      -ActualListPath "$actual_list.first"
  else
    command -v dotnet >/dev/null 2>&1 ||
      die "Windows packaging requires built-in PowerShell or a .NET SDK"
    dotnet_major="$(dotnet --version | cut -d. -f1)"
    [[ "$dotnet_major" =~ ^[0-9]+$ && "$dotnet_major" -ge 6 ]] ||
      die "Windows packaging requires .NET SDK 6 or newer"
    if [[ ! -f "$helper_dir/build/MayhemDeterministicZip.dll" ]]; then
      dotnet build \
        "$helper_project" \
        --configuration Release \
        --nologo \
        --output "$helper_dir/build" \
        --verbosity quiet >/dev/null
    fi
    dotnet "$helper_dir/build/MayhemDeterministicZip.dll" \
      "$work_dir" "$file_list" "$output" "$epoch" "$actual_list"
    dotnet "$helper_dir/build/MayhemDeterministicZip.dll" \
      "$work_dir" "$file_list" "$first" "$epoch" "$actual_list.first"
  fi

  validate_deterministic_windows_zip "$output" "$file_list"
  validate_deterministic_windows_zip "$first" "$file_list"
  cmp "$output" "$first" >/dev/null ||
    die ".NET ZIP backend did not produce deterministic bytes"
  rm -f "$first"
  cmp "$file_list" "$actual_list" >/dev/null ||
    die ".NET ZIP backend changed the canonical archive inventory"
  rm -f "$actual_list.first"
}

validate_deterministic_windows_zip() {
  local archive="$1"
  local expected_list="$2"

  WINDOWS_ZIP_ARCHIVE="$archive" \
    WINDOWS_ZIP_EXPECTED_LIST="$expected_list" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';

const archive = fs.readFileSync(process.env.WINDOWS_ZIP_ARCHIVE);
const expected = fs.readFileSync(
  process.env.WINDOWS_ZIP_EXPECTED_LIST,
  'utf8',
).split('\n').filter(Boolean);
const fail = (message) => {
  throw new Error(`deterministic Windows ZIP validation failed: ${message}`);
};
const u16 = (offset) => {
  if (offset < 0 || offset + 2 > archive.length) fail('truncated u16');
  return archive.readUInt16LE(offset);
};
const u32 = (offset) => {
  if (offset < 0 || offset + 4 > archive.length) fail('truncated u32');
  return archive.readUInt32LE(offset);
};
const eocd = archive.length - 22;
if (eocd < 0 || u32(eocd) !== 0x06054b50) fail('missing exact EOCD');
if (u16(eocd + 4) !== 0 || u16(eocd + 6) !== 0) fail('multi-disk archive');
const count = u16(eocd + 10);
if (count === 0xffff || u16(eocd + 8) !== count) fail('ZIP64 entry count');
if (u16(eocd + 20) !== 0) fail('archive comment');
const centralSize = u32(eocd + 12);
const centralOffset = u32(eocd + 16);
if (centralOffset === 0xffffffff ||
    centralSize === 0xffffffff ||
    centralOffset + centralSize !== eocd) {
  fail('invalid or ZIP64 central directory bounds');
}
if (count !== expected.length) fail('entry count differs from canonical inventory');

let cursor = centralOffset;
const ranges = [];
const actual = [];
const exact = new Set();
const portable = new Set();
for (let index = 0; index < count; index += 1) {
  if (u32(cursor) !== 0x02014b50) fail('invalid central header');
  const madeBy = u16(cursor + 4);
  const needed = u16(cursor + 6);
  const flags = u16(cursor + 8);
  const method = u16(cursor + 10);
  const crc = u32(cursor + 16);
  const compressed = u32(cursor + 20);
  const uncompressed = u32(cursor + 24);
  const nameLength = u16(cursor + 28);
  const extraLength = u16(cursor + 30);
  const commentLength = u16(cursor + 32);
  const disk = u16(cursor + 34);
  const external = u32(cursor + 38);
  const localOffset = u32(cursor + 42);
  if (needed > 20 || method !== 0) fail('unsupported ZIP feature');
  if ((flags & ~0x0800) !== 0) fail('data descriptor or unsupported ZIP flag');
  if (compressed !== uncompressed) fail('stored entry size mismatch');
  if (compressed === 0xffffffff ||
      uncompressed === 0xffffffff ||
      localOffset === 0xffffffff) {
    fail('ZIP64 entry');
  }
  if (extraLength !== 0 || commentLength !== 0 || disk !== 0) {
    fail('non-canonical entry metadata');
  }
  const nameStart = cursor + 46;
  const nameEnd = nameStart + nameLength;
  if (nameEnd > eocd) fail('truncated central name');
  const rawName = archive.subarray(nameStart, nameEnd);
  if ([...rawName].some((value) => value < 0x20 || value > 0x7e)) {
    fail('non-ASCII entry name');
  }
  const name = rawName.toString('ascii');
  const directory = name.endsWith('/');
  const host = madeBy >>> 8;
  const dosDirectory = (external & 0x10) !== 0;
  if (host === 3) {
    const kind = (external >>> 16) & 0xf000;
    if ((directory && kind !== 0x4000) ||
        (!directory && kind !== 0 && kind !== 0x8000)) {
      fail('link, special file, or ambiguous Unix type');
    }
  } else if (directory !== dosDirectory) {
    fail('ambiguous DOS file type');
  }
  const folded = name.toLowerCase();
  if (exact.has(name) || portable.has(folded)) {
    fail('duplicate or case-colliding entry');
  }
  exact.add(name);
  portable.add(folded);
  actual.push(name);

  if (u32(localOffset) !== 0x04034b50) fail('invalid local header');
  const localFlags = u16(localOffset + 6);
  const localMethod = u16(localOffset + 8);
  const localCrc = u32(localOffset + 14);
  const localCompressed = u32(localOffset + 18);
  const localUncompressed = u32(localOffset + 22);
  const localNameLength = u16(localOffset + 26);
  const localExtraLength = u16(localOffset + 28);
  if (localFlags !== flags ||
      localMethod !== method ||
      localCrc !== crc ||
      localCompressed !== compressed ||
      localUncompressed !== uncompressed ||
      localExtraLength !== 0) {
    fail('local header differs from central header');
  }
  const localNameStart = localOffset + 30;
  const localNameEnd = localNameStart + localNameLength;
  if (localNameLength !== nameLength ||
      localNameEnd > centralOffset ||
      !archive.subarray(localNameStart, localNameEnd).equals(rawName)) {
    fail('local entry name differs from central entry');
  }
  const dataEnd = localNameEnd + compressed;
  if (dataEnd > centralOffset) fail('entry overlaps central directory');
  ranges.push([localOffset, dataEnd]);
  cursor = nameEnd;
}
if (cursor !== centralOffset + centralSize) fail('trailing central records');
ranges.sort((left, right) => left[0] - right[0]);
for (let index = 1; index < ranges.length; index += 1) {
  if (ranges[index - 1][1] > ranges[index][0]) fail('overlapping entries');
}
if (actual.some((name, index) => name !== expected[index])) {
  fail('entry order or inventory differs from canonical list');
}
NODE
}

publish_release_stage_outputs() {
  local stage_dir="$1"
  local work_dir="$2"
  local output_dir="$3"
  local archive_basename="$4"
  local target="$5"
  local epoch="$6"

  normalize_release_stage "$stage_dir" "$epoch"
  create_deterministic_release_archive \
    "$stage_dir" \
    "$work_dir" \
    "$output_dir" \
    "$archive_basename" \
    "$target" \
    "$epoch"
  ARCHIVE_HASH="$(sha256_file "$ARCHIVE")"
  rm -f "$ARCHIVE.sha256"
  printf '%s  %s\n' "$ARCHIVE_HASH" "$(basename "$ARCHIVE")" >"$ARCHIVE.sha256"
  rm -f \
    "$output_dir/$archive_basename.SHA256SUMS" \
    "$output_dir/$archive_basename.manifest.json"
  cp "$stage_dir/SHA256SUMS" "$output_dir/$archive_basename.SHA256SUMS"
  cp "$stage_dir/manifest.json" "$output_dir/$archive_basename.manifest.json"
  publish_managed_verifier_artifacts "$stage_dir" "$output_dir"
  RELEASE_OUTPUT_ROOT="$output_dir" \
    RELEASE_ARCHIVE_NAME="$(basename "$ARCHIVE")" \
    RELEASE_ARCHIVE_BASENAME="$archive_basename" \
    MANAGED_EXECUTABLE_NAME="$(managed_verifier_executable_name "$VERSION" "$TARGET")" \
    MANAGED_MANIFEST_NAME="$(managed_verifier_manifest_name "$VERSION" "$TARGET")" \
    RELEASE_TIMESTAMP_EPOCH="$epoch" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_OUTPUT_ROOT);
const archiveName = process.env.RELEASE_ARCHIVE_NAME;
const executableName = process.env.MANAGED_EXECUTABLE_NAME;
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
const files = [
  archiveName,
  `${archiveName}.sha256`,
  `${process.env.RELEASE_ARCHIVE_BASENAME}.SHA256SUMS`,
  `${process.env.RELEASE_ARCHIVE_BASENAME}.manifest.json`,
  executableName,
  process.env.MANAGED_MANIFEST_NAME,
];
for (const name of files) {
  const file = path.join(root, name);
  const stat = fs.lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink()) {
    throw new Error(`published release output is not a regular file: ${name}`);
  }
  fs.chmodSync(file, name === executableName ? 0o755 : 0o644);
  fs.utimesSync(file, epoch, epoch);
}
NODE
}

normalize_release_signature_output() {
  local signature="$1"
  local epoch="$2"

  RELEASE_SIGNATURE_OUTPUT="$signature" \
    RELEASE_TIMESTAMP_EPOCH="$epoch" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const signature = path.resolve(process.env.RELEASE_SIGNATURE_OUTPUT);
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
const stat = fs.lstatSync(signature);
if (!stat.isFile() || stat.isSymbolicLink()) {
  throw new Error('release signature output must be a regular non-symlink file');
}
fs.chmodSync(signature, 0o644);
fs.utimesSync(signature, epoch, epoch);
NODE
}

snapshot_canonical_release_key() {
  local source="$1"
  local output="$2"
  local expected_key_id="$3"
  local expected_created_at="${4:-}"

  [[ -f "$source" && ! -L "$source" ]] ||
    die "canonical release public key must be a regular non-symlink file: $source"
  rm -f "$output"
  RELEASE_KEY_SOURCE="$source" \
    RELEASE_KEY_SNAPSHOT="$output" \
    RELEASE_EXPECTED_KEY_ID="$expected_key_id" \
    RELEASE_EXPECTED_KEY_CREATED_AT="$expected_created_at" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const source = path.resolve(process.env.RELEASE_KEY_SOURCE);
const output = path.resolve(process.env.RELEASE_KEY_SNAPSHOT);
const before = fs.lstatSync(source);
if (!before.isFile() || before.isSymbolicLink() ||
    before.size === 0 || before.size > 64 * 1024) {
  throw new Error('canonical release public key must be a bounded regular non-symlink file');
}
const noFollow = fs.constants.O_NOFOLLOW || 0;
let descriptor;
let outputDescriptor;
try {
  descriptor = fs.openSync(source, fs.constants.O_RDONLY | noFollow);
  const opened = fs.fstatSync(descriptor);
  if (!opened.isFile() ||
      opened.dev !== before.dev ||
      opened.ino !== before.ino ||
      opened.size !== before.size) {
    throw new Error('canonical release public key changed while it was opened');
  }
  const bytes = fs.readFileSync(descriptor);
  if (bytes.length !== opened.size) {
    throw new Error('canonical release public key changed while it was snapshotted');
  }
  const key = JSON.parse(bytes.toString('utf8'));
  const expectedFields = ['key_id', 'alg', 'public_key', 'status', 'created_at'];
  if (key === null ||
      typeof key !== 'object' ||
      Array.isArray(key) ||
      JSON.stringify(Object.keys(key)) !== JSON.stringify(expectedFields) ||
      key.key_id !== process.env.RELEASE_EXPECTED_KEY_ID ||
      !/^[A-Za-z0-9._-]{1,128}$/.test(key.key_id) ||
      key.alg !== 'ed25519' ||
      !/^[0-9a-f]{64}$/.test(key.public_key) ||
      key.status !== 'active' ||
      typeof key.created_at !== 'string' ||
      key.created_at.length === 0) {
    throw new Error('canonical release public key record is invalid or has the wrong key id');
  }
  const expectedCreatedAt = process.env.RELEASE_EXPECTED_KEY_CREATED_AT;
  if (expectedCreatedAt && key.created_at !== expectedCreatedAt) {
    throw new Error('canonical release public key created_at does not match the expected value');
  }
  outputDescriptor = fs.openSync(
    output,
    fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_EXCL,
    0o600,
  );
  fs.writeFileSync(outputDescriptor, bytes);
  fs.fsyncSync(outputDescriptor);
} finally {
  if (outputDescriptor !== undefined) fs.closeSync(outputDescriptor);
  if (descriptor !== undefined) fs.closeSync(descriptor);
}
NODE
}

verify_release_signature_output() {
  local manifest="$1"
  local signature="$2"
  local key_record="$3"
  local expected_key_id="$4"

  for input in "$manifest" "$signature" "$key_record"; do
    [[ -f "$input" && ! -L "$input" ]] ||
      die "release signature verification input must be a regular non-symlink file: $input"
  done
  RELEASE_MANIFEST_OUTPUT="$manifest" \
    RELEASE_SIGNATURE_OUTPUT="$signature" \
    RELEASE_KEY_SOURCE="$key_record" \
    RELEASE_EXPECTED_KEY_ID="$expected_key_id" \
    node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const readBounded = (input, maximum, label) => {
  const resolved = path.resolve(input);
  const stat = fs.lstatSync(resolved);
  if (!stat.isFile() || stat.isSymbolicLink() ||
      stat.size === 0 || stat.size > maximum) {
    throw new Error(`${label} must be a bounded regular non-symlink file`);
  }
  return fs.readFileSync(resolved);
};
const manifestPath = path.resolve(process.env.RELEASE_MANIFEST_OUTPUT);
const manifestBytes = readBounded(manifestPath, 64 * 1024 * 1024, 'release manifest');
const signature = JSON.parse(
  readBounded(process.env.RELEASE_SIGNATURE_OUTPUT, 64 * 1024, 'release signature')
    .toString('utf8'),
);
const key = JSON.parse(
  readBounded(process.env.RELEASE_KEY_SOURCE, 64 * 1024, 'release public key')
    .toString('utf8'),
);
const manifest = JSON.parse(manifestBytes.toString('utf8'));
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
  throw new Error('release signer emitted a detached signature with an unexpected schema');
}
if (!exactKeys(key, ['key_id', 'alg', 'public_key', 'status', 'created_at'])) {
  throw new Error('release public key record has an unexpected schema');
}
if (signature.schema_version !== 1 ||
    signature.alg !== 'ed25519' ||
    signature.signed_path !== path.basename(manifestPath) ||
    signature.key_id !== process.env.RELEASE_EXPECTED_KEY_ID ||
    signature.key_id !== key.key_id ||
    signature.public_key !== key.public_key ||
    key.alg !== 'ed25519' ||
    key.status !== 'active' ||
    !/^[A-Za-z0-9._-]{1,128}$/.test(key.key_id) ||
    !/^[0-9a-f]{64}$/.test(key.public_key) ||
    !/^[0-9a-f]{64}$/.test(signature.sha256) ||
    !/^[0-9a-f]{128}$/.test(signature.sig)) {
  throw new Error('release signer output does not match the canonical active public key record');
}
if (manifest?.schema !== 1 ||
    manifest.name !== 'mayhem' ||
    !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(manifest.version) ||
    !/^[A-Za-z0-9._-]+$/.test(manifest.target) ||
    !/^[0-9a-f]{40}$/.test(manifest.source_git_sha) ||
    signature.signed_path !==
      `mayhem-${manifest.version}-${manifest.target}.manifest.json`) {
  throw new Error('release signature path does not bind the exact manifest identity');
}
const digest = crypto.createHash('sha256').update(manifestBytes).digest('hex');
if (signature.sha256 !== digest) {
  throw new Error('release signer output does not hash the exact manifest bytes');
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
  throw new Error('independent release manifest Ed25519 signature verification failed');
}
NODE
}

publish_release_key_record() {
  local signature="$1"
  local source="$2"
  local output="$3"
  local epoch="$4"

  [[ -f "$source" && ! -L "$source" ]] ||
    die "release public key record must be a regular non-symlink file: $source"
  rm -f "$output"
  RELEASE_SIGNATURE_OUTPUT="$signature" \
    RELEASE_KEY_SOURCE="$source" \
    RELEASE_KEY_OUTPUT="$output" \
    RELEASE_TIMESTAMP_EPOCH="$epoch" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const signature = JSON.parse(fs.readFileSync(
  path.resolve(process.env.RELEASE_SIGNATURE_OUTPUT),
  'utf8',
));
const source = path.resolve(process.env.RELEASE_KEY_SOURCE);
const output = path.resolve(process.env.RELEASE_KEY_OUTPUT);
const key = JSON.parse(fs.readFileSync(source, 'utf8'));
const exactKeys = (value, expected) =>
  value !== null &&
  typeof value === 'object' &&
  !Array.isArray(value) &&
  JSON.stringify(Object.keys(value)) === JSON.stringify(expected);
if (!exactKeys(key, ['key_id', 'alg', 'public_key', 'status', 'created_at']) ||
    key.key_id !== signature.key_id ||
    key.alg !== 'ed25519' ||
    key.public_key !== signature.public_key ||
    key.status !== 'active' ||
    !/^[0-9a-f]{64}$/.test(key.public_key)) {
  throw new Error('release public key record does not match the detached signature');
}
fs.copyFileSync(source, output, fs.constants.COPYFILE_EXCL);
fs.chmodSync(output, 0o644);
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
fs.utimesSync(output, epoch, epoch);
NODE
}

publish_verified_release_outputs() {
  local source_dir="$1"
  local output_dir="$2"
  shift 2
  local name source destination temporary
  local -a expected=("$@")

  [[ -d "$source_dir" && ! -L "$source_dir" ]] ||
    die "verified release publication source must be a real directory"
  mkdir -p "$output_dir"
  [[ -d "$output_dir" && ! -L "$output_dir" ]] ||
    die "release output directory must be a real directory: $output_dir"
  [[ "${#expected[@]}" -gt 0 ]] ||
    die "verified release publication inventory must not be empty"

  RELEASE_PUBLICATION_ROOT="$source_dir" \
    RELEASE_PUBLICATION_EXPECTED="$(printf '%s\n' "${expected[@]}")" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_PUBLICATION_ROOT);
const expected = process.env.RELEASE_PUBLICATION_EXPECTED.split('\n').filter(Boolean).sort();
const actual = fs.readdirSync(root).sort();
if (JSON.stringify(actual) !== JSON.stringify(expected)) {
  throw new Error('verified release publication inventory is not exact');
}
for (const name of expected) {
  if (!/^[A-Za-z0-9._-]+$/.test(name)) {
    throw new Error(`unsafe release publication name: ${name}`);
  }
  const stat = fs.lstatSync(path.join(root, name));
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size === 0) {
    throw new Error(`release publication input is not a non-empty regular file: ${name}`);
  }
}
NODE

  for name in "${expected[@]}"; do
    source="$source_dir/$name"
    destination="$output_dir/$name"
    temporary="$(mktemp "$output_dir/.mayhem-publish.XXXXXX")"
    cp -p "$source" "$temporary"
    mv -f "$temporary" "$destination"
  done
}

publish_managed_verifier_artifacts() {
  local stage_dir="$1"
  local output_dir="$2"
  local executable_name manifest_name source destination

  executable_name="$(managed_verifier_executable_name "$VERSION" "$TARGET")"
  manifest_name="$(managed_verifier_manifest_name "$VERSION" "$TARGET")"
  for source in "$stage_dir/$executable_name" "$stage_dir/$manifest_name"; do
    [[ -f "$source" && ! -L "$source" ]] ||
      die "managed verifier publication source must be a regular non-symlink file: $source"
    destination="$output_dir/$(basename "$source")"
    rm -f "$destination"
    cp "$source" "$destination"
  done
  chmod 0755 "$output_dir/$executable_name" 2>/dev/null || true
}

require_tracked_file() {
  local relative="$1"

  git -C "$ROOT_DIR" ls-files --error-unmatch -- "$relative" >/dev/null 2>&1 ||
    die "required release source is not tracked: $relative"
  [[ -f "$ROOT_DIR/$relative" && ! -L "$ROOT_DIR/$relative" ]] ||
    die "required release source must be a regular file: $relative"
}

copy_tracked_allowlist() {
  local dest="$1"
  shift
  local relative source copied=0

  mkdir -p "$dest"
  while IFS= read -r -d '' relative; do
    source="$ROOT_DIR/$relative"
    [[ -f "$source" && ! -L "$source" ]] ||
      die "tracked release source must be a regular file: $relative"
    mkdir -p "$dest/$(dirname "$relative")"
    cp -p "$source" "$dest/$relative"
    copied=$((copied + 1))
  done < <(git -C "$ROOT_DIR" ls-files -z -- "$@")
  [[ "$copied" -gt 0 ]] || die "release source allowlist matched no tracked files"
}

verify_intercom_release_identity() {
  local intercom_root="$1"

  command -v node >/dev/null 2>&1 ||
    die "node is required to verify the Intercom release identity"
  INTERCOM_RELEASE_ROOT="$intercom_root" node --input-type=module <<'NODE'
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const rootDir = path.resolve(process.env.INTERCOM_RELEASE_ROOT);
const verifierUrl = pathToFileURL(path.join(rootDir, 'src/release-identity.js')).href;
const { verifyStartupReleaseIdentity } = await import(verifierUrl);
verifyStartupReleaseIdentity({ rootDir });
NODE
}

write_intercom_release_metadata() {
  local intercom_root="$1"
  local output="$2"

  INTERCOM_RELEASE_ROOT="$intercom_root" \
    INTERCOM_RELEASE_METADATA="$output" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const rootDir = path.resolve(process.env.INTERCOM_RELEASE_ROOT);
const output = path.resolve(process.env.INTERCOM_RELEASE_METADATA);
const verifierUrl = pathToFileURL(path.join(rootDir, 'src/release-identity.js')).href;
const {
  createIntercomBundleManifest,
  verifyIntercomBundleManifest,
} = await import(verifierUrl);
const metadata = createIntercomBundleManifest({ rootDir });
verifyIntercomBundleManifest(metadata, { rootDir });
fs.writeFileSync(output, `${JSON.stringify(metadata)}\n`);
NODE
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

lockfile_hints_at_native_runtime_dependencies() {
  local lockfile="$1"

  INTERCOM_LOCKFILE="$lockfile" node --input-type=module <<'NODE'
import fs from 'node:fs';

const lock = JSON.parse(fs.readFileSync(process.env.INTERCOM_LOCKFILE, 'utf8'));
const packages = Object.entries(lock.packages ?? {});
const native = packages.some(([packagePath, metadata]) => {
  if (!packagePath.startsWith('node_modules/') || metadata?.dev === true) return false;
  const name = packagePath.split('/').at(-1);
  return name.endsWith('-native') ||
    Object.prototype.hasOwnProperty.call(metadata?.dependencies ?? {}, 'require-addon');
});
process.exit(native ? 0 : 1);
NODE
}

target_prebuild_name() {
  local target="$1"
  local platform arch

  case "$target" in
    *-apple-darwin) platform="darwin" ;;
    *-linux-*) platform="linux" ;;
    *-windows-*) platform="win32" ;;
    *) die "unsupported Intercom native dependency target: $target" ;;
  esac
  case "$target" in
    x86_64-*) arch="x64" ;;
    aarch64-*) arch="arm64" ;;
    *) die "unsupported Intercom native dependency architecture: $target" ;;
  esac
  printf '%s-%s\n' "$platform" "$arch"
}

finalize_intercom_native_artifacts() {
  local intercom_root="$1"
  local keep="$2"
  local target="$3"
  local native_host="$4"

  INTERCOM_RELEASE_ROOT="$intercom_root" \
    INTERCOM_PREBUILD_TARGET="$keep" \
    INTERCOM_PACKAGE_TARGET="$target" \
    INTERCOM_NATIVE_HOST="$native_host" \
    node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.INTERCOM_RELEASE_ROOT, 'node_modules');
const keep = process.env.INTERCOM_PREBUILD_TARGET;
const target = process.env.INTERCOM_PACKAGE_TARGET;
const nativeHost = process.env.INTERCOM_NATIVE_HOST;
const prebuildDirectories = [];
const nodeArtifacts = [];
const visit = (directory, enclosingPrebuild = null) => {
  const entries = fs.readdirSync(directory, { withFileTypes: true });
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    const stat = fs.lstatSync(entryPath);
    if (stat.isSymbolicLink()) {
      throw new Error(`hydrated dependency tree contains symbolic link: ${entryPath}`);
    }
    if (stat.isDirectory()) {
      const prebuild = entry.name === 'prebuilds' ? entryPath : enclosingPrebuild;
      if (entry.name === 'prebuilds') prebuildDirectories.push(entryPath);
      visit(entryPath, prebuild);
      continue;
    }
    if (!stat.isFile()) {
      throw new Error(`hydrated dependency tree contains a special file: ${entryPath}`);
    }
    if (entry.name.toLowerCase().endsWith('.node')) {
      nodeArtifacts.push({ file: entryPath, prebuild: enclosingPrebuild });
    }
  }
};
const countRegularFiles = (directory) => {
  let count = 0;
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    const stat = fs.lstatSync(entryPath);
    if (stat.isSymbolicLink()) {
      throw new Error(`native prebuild contains symbolic link: ${entryPath}`);
    }
    if (stat.isDirectory()) {
      count += countRegularFiles(entryPath);
    } else if (stat.isFile()) {
      count += 1;
    } else {
      throw new Error(`native prebuild contains a special file: ${entryPath}`);
    }
  }
  return count;
};

visit(root);
if ((prebuildDirectories.length > 0 || nodeArtifacts.length > 0) && target !== nativeHost) {
  throw new Error(
    `hydrated runtime contains native artifacts; package target ${target} ` +
    `must match native host ${nativeHost}`
  );
}
for (const prebuilds of prebuildDirectories) {
  const entries = fs.readdirSync(prebuilds, { withFileTypes: true });
  const selected = entries.find((entry) => entry.name === keep);
  if (!selected || !selected.isDirectory() || selected.isSymbolicLink()) {
    throw new Error(`native dependency ${prebuilds} has no ${keep} prebuild`);
  }
  const selectedPath = path.join(prebuilds, keep);
  if (countRegularFiles(selectedPath) === 0) {
    throw new Error(`native dependency ${prebuilds} has an empty ${keep} prebuild`);
  }
  for (const entry of entries) {
    if (entry.name !== keep) {
      fs.rmSync(path.join(prebuilds, entry.name), { recursive: true, force: false });
    }
  }
}

prebuildDirectories.length = 0;
nodeArtifacts.length = 0;
visit(root);
for (const prebuilds of prebuildDirectories) {
  const entries = fs.readdirSync(prebuilds, { withFileTypes: true });
  if (entries.length !== 1 || entries[0].name !== keep || !entries[0].isDirectory()) {
    throw new Error(`native dependency retains non-target prebuilds: ${prebuilds}`);
  }
}
for (const artifact of nodeArtifacts) {
  if (artifact.prebuild) {
    const selectedRoot = path.join(artifact.prebuild, keep);
    if (artifact.file !== selectedRoot && !artifact.file.startsWith(`${selectedRoot}${path.sep}`)) {
      throw new Error(`native addon is outside selected ${keep} prebuild: ${artifact.file}`);
    }
  }
}
NODE
}

verify_intercom_production_dependency_tree() {
  local intercom_root="$1"

  INTERCOM_RELEASE_ROOT="$intercom_root" node --input-type=module <<'NODE'
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.INTERCOM_RELEASE_ROOT);
const lock = JSON.parse(fs.readFileSync(path.join(root, 'package-lock.json'), 'utf8'));
const leakedDevPackages = Object.entries(lock.packages ?? {})
  .filter(([packagePath, metadata]) => packagePath && metadata?.dev === true)
  .map(([packagePath]) => packagePath)
  .filter((packagePath) => {
    const resolved = path.resolve(root, packagePath);
    if (!resolved.startsWith(`${root}${path.sep}`)) {
      throw new Error(`package lock path escapes Intercom root: ${packagePath}`);
    }
    return fs.existsSync(resolved);
  });
if (leakedDevPackages.length > 0) {
  throw new Error(
    `production hydration contains dev-only packages: ${leakedDevPackages.slice(0, 10).join(', ')}`
  );
}
NODE
}

hydrate_intercom_runtime_tree() {
  local dest="$2"
  local temp_root="$1"
  local target="$3"
  local hydrate_root="$temp_root/intercom-hydrate"
  local intercom_root="$hydrate_root/intercom"
  local package_hash lock_hash msb_package_hash msb_lock_hash
  local peer_package_hash peer_lock_hash native_host

  command -v npm >/dev/null 2>&1 ||
    die "npm is required to hydrate the production Intercom dependency tree"
  for required in \
    scripts/verify-intercom-dependency-topology.mjs \
    intercom/.npmrc \
    intercom/package.json \
    intercom/package-lock.json \
    intercom/contract/release.json \
    intercom/scripts/materialize-local-dependencies.mjs \
    intercom/src/release-identity.js \
    intercom/trac/msb/package.json \
    intercom/trac/msb/package-lock.json \
    intercom/trac/trac-peer/package.json \
    intercom/trac/trac-peer/package-lock.json; do
    require_tracked_file "$required"
  done

  rm -rf "$hydrate_root"
  copy_tracked_allowlist "$hydrate_root" "${INTERCOM_SOURCE_ALLOWLIST[@]}"
  verify_intercom_release_identity "$intercom_root"
  native_host="$(native_host_target)"

  if lockfile_hints_at_native_runtime_dependencies "$intercom_root/package-lock.json"; then
    [[ "$target" == "$native_host" ]] ||
      die "Intercom has native dependencies; package target $target must match native host $native_host"
  fi

  package_hash="$(sha256_file "$intercom_root/package.json")"
  lock_hash="$(sha256_file "$intercom_root/package-lock.json")"
  msb_package_hash="$(sha256_file "$intercom_root/trac/msb/package.json")"
  msb_lock_hash="$(sha256_file "$intercom_root/trac/msb/package-lock.json")"
  peer_package_hash="$(sha256_file "$intercom_root/trac/trac-peer/package.json")"
  peer_lock_hash="$(sha256_file "$intercom_root/trac/trac-peer/package-lock.json")"
  mkdir -p "$temp_root/npm-home" "$temp_root/npm-cache"
  rm -f "$temp_root/npmrc"
  touch "$temp_root/npmrc"
  log "hydrating clean production Intercom dependencies"
  (
    cd "$intercom_root"
    clean_npm_env=(
      env -i
      "HOME=$temp_root/npm-home"
      "PATH=$PATH"
      "TMPDIR=${TMPDIR:-/tmp}"
      "npm_config_cache=$temp_root/npm-cache"
      "npm_config_userconfig=$temp_root/npmrc"
    )
    "${clean_npm_env[@]}" \
      npm ci \
        --omit=dev \
        --install-links=true \
        --no-audit \
        --no-fund \
        --ignore-scripts \
        --no-bin-links
  )
  [[ -d "$intercom_root/node_modules" ]] ||
    die "npm did not produce the Intercom production dependency tree"
  [[ "$package_hash" == "$(sha256_file "$intercom_root/package.json")" ]] ||
    die "npm changed the pinned Intercom package.json"
  [[ "$lock_hash" == "$(sha256_file "$intercom_root/package-lock.json")" ]] ||
    die "npm changed the pinned Intercom package-lock.json"
  [[ "$msb_package_hash" == "$(sha256_file "$intercom_root/trac/msb/package.json")" ]] ||
    die "npm changed the pinned trac-msb package.json"
  [[ "$msb_lock_hash" == "$(sha256_file "$intercom_root/trac/msb/package-lock.json")" ]] ||
    die "npm changed the pinned trac-msb package-lock.json"
  [[ "$peer_package_hash" == "$(sha256_file "$intercom_root/trac/trac-peer/package.json")" ]] ||
    die "npm changed the pinned trac-peer package.json"
  [[ "$peer_lock_hash" == "$(sha256_file "$intercom_root/trac/trac-peer/package-lock.json")" ]] ||
    die "npm changed the pinned trac-peer package-lock.json"
  node "$intercom_root/scripts/materialize-local-dependencies.mjs" "$intercom_root"
  node "$ROOT_DIR/scripts/verify-intercom-dependency-topology.mjs" "$intercom_root"
  verify_intercom_production_dependency_tree "$intercom_root"
  finalize_intercom_native_artifacts \
    "$intercom_root" \
    "$(target_prebuild_name "$target")" \
    "$target" \
    "$native_host"
  verify_intercom_release_identity "$intercom_root"
  mkdir -p "$(dirname "$dest")"
  mv "$intercom_root" "$dest"
}

stage_runtime_assets() {
  local asset_dir="$1"
  local temp_root="$2"
  local target="$3"

  copy_tracked_allowlist "$asset_dir" "${RELEASE_ASSET_SOURCE_ALLOWLIST[@]}"
  hydrate_intercom_runtime_tree "$temp_root" "$asset_dir/intercom" "$target"
}

write_stage_checksums() {
  local stage_dir="$1"

  RELEASE_STAGE_ROOT="$stage_dir" node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_STAGE_ROOT);
const output = path.join(root, 'SHA256SUMS');
if (fs.existsSync(output)) throw new Error(`checksum metadata already exists: ${output}`);
const safe = (relative) => {
  if (!/^[\x20-\x7e]+$/.test(relative) ||
      relative.startsWith('/') ||
      relative.includes('\\') ||
      /[<>:"|?*]/.test(relative) ||
      relative.split('/').some((part) =>
        part.length === 0 || part === '.' || part === '..' ||
        part.endsWith('.') || part.endsWith(' '))) {
    throw new Error(`unsafe staged release path: ${JSON.stringify(relative)}`);
  }
};
const files = [];
const visit = (directory, parent = []) => {
  const entries = fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
  for (const entry of entries) {
    const parts = [...parent, entry.name];
    const relative = parts.join('/');
    safe(relative);
    const entryPath = path.join(directory, entry.name);
    const stat = fs.lstatSync(entryPath);
    if (stat.isSymbolicLink()) throw new Error(`staged release path is a symbolic link: ${relative}`);
    if (stat.isDirectory()) {
      visit(entryPath, parts);
    } else if (stat.isFile() && relative !== 'SHA256SUMS') {
      files.push(relative);
    } else if (!stat.isFile()) {
      throw new Error(`staged release path is not a regular file or directory: ${relative}`);
    }
  }
};
visit(root);
files.sort();
const lines = files.map((relative) => {
  const digest = crypto.createHash('sha256')
    .update(fs.readFileSync(path.join(root, relative)))
    .digest('hex');
  return `${digest}  ${relative}\n`;
});
fs.writeFileSync(output, lines.join(''), { flag: 'wx', mode: 0o644 });
NODE
}

write_release_manifest() {
  local stage_dir="$1"
  local intercom_metadata="$2"
  local output="$3"
  local bin_names

  bin_names="$(printf '%s\n' "${BINS[@]}")"
  RELEASE_STAGE_ROOT="$stage_dir" \
    RELEASE_INTERCOM_METADATA="$intercom_metadata" \
    RELEASE_MANIFEST_OUTPUT="$output" \
    RELEASE_VERSION="$VERSION" \
    RELEASE_TARGET="$TARGET" \
    RELEASE_BUILT_AT="$BUILT_AT" \
    RELEASE_SOURCE_GIT_SHA="$SOURCE_GIT_SHA" \
    RELEASE_BIN_NAMES="$bin_names" \
    RELEASE_BIN_EXT="$BIN_EXT" \
    node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_STAGE_ROOT);
const output = path.resolve(process.env.RELEASE_MANIFEST_OUTPUT);
for (const metadata of ['manifest.json', 'SHA256SUMS']) {
  if (fs.existsSync(path.join(root, metadata))) {
    throw new Error(`release metadata must not exist before inventory: ${metadata}`);
  }
}
const safe = (relative) => {
  if (!/^[\x20-\x7e]+$/.test(relative) ||
      relative.startsWith('/') ||
      relative.includes('\\') ||
      /[<>:"|?*]/.test(relative) ||
      relative.split('/').some((part) =>
        part.length === 0 || part === '.' || part === '..' ||
        part.endsWith('.') || part.endsWith(' '))) {
    throw new Error(`unsafe staged release path: ${JSON.stringify(relative)}`);
  }
};
const assets = [];
const visit = (directory, parent = []) => {
  const entries = fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
  for (const entry of entries) {
    const parts = [...parent, entry.name];
    const relative = parts.join('/');
    safe(relative);
    const entryPath = path.join(directory, entry.name);
    const stat = fs.lstatSync(entryPath);
    if (stat.isSymbolicLink()) throw new Error(`staged release path is a symbolic link: ${relative}`);
    if (stat.isDirectory()) {
      visit(entryPath, parts);
    } else if (stat.isFile()) {
      assets.push({
        path: relative,
        sha256: crypto.createHash('sha256').update(fs.readFileSync(entryPath)).digest('hex'),
      });
    } else {
      throw new Error(`staged release path is not a regular file or directory: ${relative}`);
    }
  }
};
visit(root);
assets.sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
if (assets.length === 0) throw new Error('release payload inventory must not be empty');
const byPath = new Map(assets.map((asset) => [asset.path, asset]));
if (!/^[0-9a-f]{40}$/.test(process.env.RELEASE_SOURCE_GIT_SHA)) {
  throw new Error('release source_git_sha must be an exact lowercase 40-hex commit id');
}
const binaryNames = process.env.RELEASE_BIN_NAMES.split('\n').filter(Boolean);
const binaries = binaryNames.map((baseName) => {
  const name = `${baseName}${process.env.RELEASE_BIN_EXT}`;
  const binaryPath = `bin/${name}`;
  const asset = byPath.get(binaryPath);
  if (!asset) throw new Error(`release binary is missing from payload inventory: ${binaryPath}`);
  return { name, path: binaryPath, sha256: asset.sha256 };
});
const intercom = JSON.parse(fs.readFileSync(process.env.RELEASE_INTERCOM_METADATA, 'utf8'));
const boundVersion = process.env.RELEASE_VERSION;
if (!/^[0-9]+\.[0-9]+\.[0-9]+$/.test(boundVersion) ||
    boundVersion !== intercom.release_version) {
  throw new Error(
    `outer release version ${process.env.RELEASE_VERSION} does not bind to ` +
    `Intercom release version ${intercom.release_version}`
  );
}
for (const intercomAsset of intercom.assets ?? []) {
  const outerAsset = byPath.get(intercomAsset.path);
  if (!outerAsset || outerAsset.sha256 !== intercomAsset.sha256) {
    throw new Error(`Intercom asset is not identically covered by outer manifest: ${intercomAsset.path}`);
  }
}
const manifest = {
  schema: 1,
  name: 'mayhem',
  version: process.env.RELEASE_VERSION,
  target: process.env.RELEASE_TARGET,
  built_at_utc: process.env.RELEASE_BUILT_AT,
  source_git_sha: process.env.RELEASE_SOURCE_GIT_SHA,
  binaries,
  assets,
  intercom,
};
fs.writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`, {
  flag: 'wx',
  mode: 0o644,
});
NODE
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

native_host_target() {
  local host rust_host

  host="$(host_target)"
  case "$host" in
    x86_64-apple-darwin)
      if [[ "$(sysctl -n sysctl.proc_translated 2>/dev/null || printf '0\n')" == "1" ]]; then
        printf 'aarch64-apple-darwin\n'
      else
        printf '%s\n' "$host"
      fi
      ;;
    x86_64-pc-windows-msvc | aarch64-pc-windows-msvc)
      rust_host="$(rustc -vV | sed -n 's/^host: //p')"
      case "$rust_host" in
        x86_64-pc-windows-msvc | aarch64-pc-windows-msvc)
          printf '%s\n' "$rust_host"
          ;;
        *)
          die "unsupported native Windows Rust host: ${rust_host:-unknown}"
          ;;
      esac
      ;;
    *)
      printf '%s\n' "$host"
      ;;
  esac
}

llama_cpp_feature_name() {
  local token
  token="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"

  case "$token" in
    cpu | none) printf '\n' ;;
    cuda | llama-cpp-cuda | mayhem-cli/llama-cpp-cuda)
      printf 'mayhem-cli/llama-cpp-cuda\n'
      ;;
    vulkan | llama-cpp-vulkan | mayhem-cli/llama-cpp-vulkan)
      printf 'mayhem-cli/llama-cpp-vulkan\n'
      ;;
    openmp | llama-cpp-openmp | mayhem-cli/llama-cpp-openmp)
      printf 'mayhem-cli/llama-cpp-openmp\n'
      ;;
    static-openmp | llama-cpp-static-openmp | mayhem-cli/llama-cpp-static-openmp)
      printf 'mayhem-cli/llama-cpp-static-openmp\n'
      ;;
    *)
      die "unknown MAYHEM_LLAMA_CPP_FEATURES entry '$1' (expected cuda, vulkan, openmp, static-openmp, or cpu)"
      ;;
  esac
}

llama_cpp_cuda_toolkit_usable() {
  local candidate resolved nvcc_usable=0
  local -a candidates=()

  [[ -n "${CUDACXX:-}" ]] && candidates+=("$CUDACXX")
  [[ -n "${CUDA_HOME:-}" ]] && candidates+=("$CUDA_HOME/bin/nvcc")
  [[ -n "${CUDA_PATH:-}" ]] && candidates+=("$CUDA_PATH/bin/nvcc")
  candidates+=("/usr/local/cuda/bin/nvcc" "/opt/cuda/bin/nvcc" "nvcc")

  for candidate in "${candidates[@]}"; do
    resolved="$candidate"
    if [[ "$candidate" != */* ]]; then
      resolved="$(command -v "$candidate" 2>/dev/null || true)"
    fi
    [[ -n "$resolved" && -x "$resolved" ]] || continue
    if "$resolved" --version >/dev/null 2>&1; then
      nvcc_usable=1
      break
    fi
  done
  [[ "$nvcc_usable" == "1" ]] || return 1

  if [[ "$(uname -s)" == "Linux" ]]; then
    llama_cpp_cuda_toolkit_root >/dev/null &&
    llama_cpp_cuda_library_dirs >/dev/null
  fi
}

llama_cpp_cuda_toolkit_root() {
  local candidate resolved
  local -a candidates=()

  for candidate in \
    "${CUDA_PATH:-}" \
    "${CUDA_HOME:-}" \
    "${CUDA_ROOT:-}" \
    "${CUDA_TOOLKIT_ROOT_DIR:-}"; do
    [[ -n "$candidate" ]] && candidates+=("$candidate")
  done
  for candidate in \
    "${CUDACXX:-}" \
    "${CUDA_HOME:+$CUDA_HOME/bin/nvcc}" \
    "${CUDA_PATH:+$CUDA_PATH/bin/nvcc}" \
    /usr/local/cuda/bin/nvcc \
    /opt/cuda/bin/nvcc \
    "$(command -v nvcc 2>/dev/null || true)"; do
    [[ -n "$candidate" && -x "$candidate" ]] || continue
    resolved="$(cd -P "$(dirname "$candidate")/.." && pwd)"
    candidates+=("$resolved")
  done
  candidates+=(/usr/local/cuda /opt/cuda /usr)

  for candidate in "${candidates[@]}"; do
    [[ -n "$candidate" &&
      -r "$candidate/include/cuda.h" &&
      -x "$candidate/bin/nvcc" ]] || continue
    printf '%s\n' "$candidate"
    return 0
  done
  return 1
}

llama_cpp_cuda_library_dirs() {
  local candidate root triplet joined="" found_any=0
  local found_cudart=0 found_cublas=0 found_cublas_lt=0 found_culibos=0
  local -a candidates=() roots=()

  if [[ -n "${CUDA_LIBRARY_PATH:-}" ]]; then
    IFS=':' read -r -a candidates <<< "$CUDA_LIBRARY_PATH"
  fi
  for root in \
    "${CUDA_HOME:-}" \
    "${CUDA_PATH:-}" \
    "${CUDA_ROOT:-}" \
    "${CUDA_TOOLKIT_ROOT_DIR:-}" \
    /usr/local/cuda \
    /opt/cuda; do
    [[ -n "$root" ]] && roots+=("$root")
  done
  for root in "${roots[@]}"; do
    candidates+=(
      "$root/lib64"
      "$root/lib"
      "$root/targets/x86_64-linux/lib"
      "$root/targets/aarch64-linux/lib"
    )
  done
  for root in /usr/local/cuda-*; do
    [[ -d "$root" ]] || continue
    candidates+=(
      "$root/lib64"
      "$root/lib"
      "$root/targets/x86_64-linux/lib"
      "$root/targets/aarch64-linux/lib"
    )
  done
  triplet="$(gcc -print-multiarch 2>/dev/null || true)"
  [[ -n "$triplet" ]] && candidates+=("/usr/lib/$triplet")
  candidates+=(
    "/usr/lib/$(uname -m)-linux-gnu"
    /usr/lib64
    /usr/lib
  )

  for candidate in "${candidates[@]}"; do
    [[ -n "$candidate" && -d "$candidate" ]] || continue
    case ":$joined:" in
      *":$candidate:"*) continue ;;
    esac
    [[ -r "$candidate/libcudart_static.a" ]] && found_cudart=1
    [[ -r "$candidate/libcublas_static.a" ]] && found_cublas=1
    [[ -r "$candidate/libcublasLt_static.a" ]] && found_cublas_lt=1
    [[ -r "$candidate/libculibos.a" ]] && found_culibos=1
    if [[ -r "$candidate/libcudart_static.a" ||
      -r "$candidate/libcublas_static.a" ||
      -r "$candidate/libcublasLt_static.a" ||
      -r "$candidate/libculibos.a" ]]; then
      joined="${joined:+$joined:}$candidate"
      found_any=1
    fi
  done

  [[ "$found_any" == "1" &&
    "$found_cudart" == "1" &&
    "$found_cublas" == "1" &&
    "$found_cublas_lt" == "1" &&
    "$found_culibos" == "1" ]] || return 1
  printf '%s\n' "$joined"
}

export_llama_cpp_cuda_link_search() {
  local library_dirs="$1"
  local library_dir flag separator=$'\x1f'
  local -a required_dirs=()

  IFS=':' read -r -a required_dirs <<< "$library_dirs"
  for library_dir in "${required_dirs[@]}"; do
    [[ -d "$library_dir" ]] || continue
    flag="-Lnative=$library_dir"
    if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
      case "${separator}${CARGO_ENCODED_RUSTFLAGS}${separator}" in
        *"${separator}${flag}${separator}"*) ;;
        *)
          export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}${separator}${flag}"
          ;;
      esac
    else
      case " ${RUSTFLAGS:-} " in
        *" $flag "*) ;;
        *) export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$flag" ;;
      esac
    fi
  done
}

llama_cpp_vulkan_toolkit_usable() {
  local library_dir

  if command -v pkg-config >/dev/null 2>&1 &&
    pkg-config --exists vulkan >/dev/null 2>&1; then
    return 0
  fi

  [[ -n "${VULKAN_SDK:-}" ]] || return 1
  [[ -r "$VULKAN_SDK/include/vulkan/vulkan.h" ]] || return 1
  for library_dir in "$VULKAN_SDK/lib" "$VULKAN_SDK/lib64"; do
    if compgen -G "$library_dir/libvulkan.so*" >/dev/null; then
      return 0
    fi
  done
  return 1
}

normalize_linux_arch() {
  case "$1" in
    x86_64 | amd64) printf 'x86_64\n' ;;
    aarch64 | arm64) printf 'aarch64\n' ;;
    *) printf '%s\n' "$1" ;;
  esac
}

linux_llama_cpp_features() {
  local os="$1"
  local target_arch host_arch raw token feature features=""
  local -a tokens=()

  [[ "$os" == "Linux" ]] || return 0
  target_arch="$(normalize_linux_arch "$2")"
  host_arch="$(normalize_linux_arch "${3:-$2}")"
  case "$target_arch" in
    x86_64 | aarch64) ;;
    *) return 0 ;;
  esac

  raw="${MAYHEM_LLAMA_CPP_FEATURES:-}"
  if [[ -n "${raw//[[:space:],;]/}" ]]; then
    IFS=',; ' read -r -a tokens <<< "$raw"
    for token in "${tokens[@]}"; do
      [[ -n "$token" ]] || continue
      feature="$(llama_cpp_feature_name "$token")"
      [[ -n "$feature" ]] || continue
      case "$feature" in
        mayhem-cli/llama-cpp-cuda)
          llama_cpp_cuda_toolkit_usable ||
            die "llama.cpp CUDA source build requested, but a working nvcc was not found; install CUDA Toolkit or set MAYHEM_LLAMA_CPP_FEATURES=cpu"
          ;;
        mayhem-cli/llama-cpp-vulkan)
          llama_cpp_vulkan_toolkit_usable ||
            die "llama.cpp Vulkan source build requested, but Vulkan headers and loader were not found; install the Vulkan SDK or set MAYHEM_LLAMA_CPP_FEATURES=cpu"
          ;;
      esac
      case ",$features," in
        *",$feature,"*) ;;
        *) features="${features:+$features,}$feature" ;;
      esac
    done
    printf '%s\n' "$features"
    return 0
  fi

  # A host toolkit does not prove that a cross-compiled target can use it.
  if [[ "$target_arch" != "$host_arch" ]]; then
    return 0
  fi
  if llama_cpp_cuda_toolkit_usable; then
    printf 'mayhem-cli/llama-cpp-cuda\n'
  elif llama_cpp_vulkan_toolkit_usable; then
    printf 'mayhem-cli/llama-cpp-vulkan\n'
  fi
}

if [[ "${MAYHEM_PACKAGE_RELEASE_SOURCE_ONLY:-0}" == "1" ]]; then
  return 0 2>/dev/null || exit 0
fi

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
    --unsigned-layout)
      UNSIGNED_LAYOUT=1
      shift
      ;;
    --verifier-identity-file)
      [[ $# -ge 2 ]] || die "--verifier-identity-file requires a value"
      VERIFIER_IDENTITY_FILE="$2"
      shift 2
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
    VERSION="$WORKSPACE_VERSION"
  elif git -C "$ROOT_DIR" describe --tags --always --dirty >/dev/null 2>&1; then
    VERSION="$(git -C "$ROOT_DIR" describe --tags --always --dirty)"
  else
    VERSION="dev"
  fi
fi

if [[ -z "$TARGET" ]]; then
  TARGET="$(native_host_target)"
fi
validate_release_artifact_identity "$VERSION" "$TARGET"
SIGNED_RELEASE=0
[[ -n "$RELEASE_SEED_FILE" ]] && SIGNED_RELEASE=1
validate_release_mode "$SIGNED_RELEASE"
SOURCE_GIT_SHA="$(clean_source_git_sha "$ROOT_DIR")"
RELEASE_EPOCH="$(resolve_release_epoch)"
BUILT_AT="$(release_epoch_iso8601 "$RELEASE_EPOCH")"
export SOURCE_DATE_EPOCH="$RELEASE_EPOCH"

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

if [[ "$SIGNED_RELEASE" == "1" ]]; then
  [[ -n "$RELEASE_KEY_ID" ]] ||
    die "--release-key-id is required with --release-seed-file"
  prepare_fresh_signed_binary_outputs "$RELEASE_DIR"
fi

log "verifying Intercom release identity"
verify_intercom_release_identity "$ROOT_DIR/intercom"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  llama_cpp_features=""
  cuda_library_dirs=""
  cuda_toolkit_root=""
  cargo_args=(build --release --workspace --bins)
  [[ "$SIGNED_RELEASE" == "1" ]] && cargo_args+=(--locked)
  if [[ "$TARGET_SET" -eq 1 ]]; then
    cargo_args+=(--target "$TARGET")
  fi
  case "$TARGET" in
    *-apple-darwin) cargo_args+=(--features mayhem-cli/llama-cpp-metal) ;;
    x86_64-*-linux-* | aarch64-*-linux-*)
      target_arch="${TARGET%%-*}"
      llama_cpp_features="$(
        linux_llama_cpp_features Linux "$target_arch" "$(uname -m)"
      )"
      if [[ -n "$llama_cpp_features" ]]; then
        log "building llama.cpp provider feature(s): $llama_cpp_features"
        cargo_args+=(--features "$llama_cpp_features")
      else
        log "building llama.cpp CPU fallback; install a usable CUDA or Vulkan toolkit, or set MAYHEM_LLAMA_CPP_FEATURES explicitly"
      fi
      ;;
  esac
  if [[ ",$llama_cpp_features," == *,mayhem-cli/llama-cpp-cuda,* ]]; then
    cuda_toolkit_root="$(llama_cpp_cuda_toolkit_root)" ||
      die "llama.cpp CUDA source build could not identify a complete CUDA Toolkit root"
    cuda_library_dirs="$(llama_cpp_cuda_library_dirs)" ||
      die "llama.cpp CUDA source build requires libcudart_static.a, libcublas_static.a, libcublasLt_static.a, and libculibos.a; install the CUDA development libraries or select another backend"
    export CUDA_PATH="$cuda_toolkit_root"
    case ":${CUDA_LIBRARY_PATH:-}:" in
      *":$cuda_library_dirs:"*) ;;
      *)
        export CUDA_LIBRARY_PATH="${cuda_library_dirs}${CUDA_LIBRARY_PATH:+:$CUDA_LIBRARY_PATH}"
        ;;
    esac
    log "using validated CUDA Toolkit $cuda_toolkit_root with static libraries from $cuda_library_dirs"
    export_llama_cpp_cuda_link_search "$cuda_library_dirs"
  fi
  log "building release binaries"
  (cd "$ROOT_DIR" && cargo "${cargo_args[@]}")
fi
verify_clean_source_git_sha "$SOURCE_GIT_SHA" "$ROOT_DIR"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-package.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
chmod 0700 "$TMP_DIR"

ARCHIVE_BASENAME="mayhem-${VERSION}-${TARGET}"
STAGE_DIR="$TMP_DIR/$ARCHIVE_BASENAME"
ASSET_DIR="$STAGE_DIR/share/mayhem"
PUBLICATION_DIR="$TMP_DIR/publication"
mkdir -p "$STAGE_DIR/bin" "$PUBLICATION_DIR"

stage_release_binaries "$RELEASE_DIR" "$STAGE_DIR"

require_tracked_file README.md
require_tracked_file RULES.md
copy_tracked_allowlist "$STAGE_DIR" README.md RULES.md

stage_runtime_assets "$ASSET_DIR" "$TMP_DIR" "$TARGET"
INTERCOM_RELEASE_METADATA="$TMP_DIR/intercom-release-metadata.json"
write_intercom_release_metadata "$ASSET_DIR/intercom" "$INTERCOM_RELEASE_METADATA"

verify_clean_source_git_sha "$SOURCE_GIT_SHA" "$ROOT_DIR"
stage_managed_verifier_artifacts "$STAGE_DIR" "$VERIFIER_IDENTITY_FILE"
MANIFEST="$STAGE_DIR/manifest.json"
write_release_manifest "$STAGE_DIR" "$INTERCOM_RELEASE_METADATA" "$MANIFEST"

write_stage_checksums "$STAGE_DIR"
publish_release_stage_outputs \
  "$STAGE_DIR" \
  "$TMP_DIR" \
  "$PUBLICATION_DIR" \
  "$ARCHIVE_BASENAME" \
  "$TARGET" \
  "$RELEASE_EPOCH"

if [[ "$SIGNED_RELEASE" == "1" ]]; then
  verify_clean_source_git_sha "$SOURCE_GIT_SHA" "$ROOT_DIR"
  RELEASE_SIGNER_BIN="$STAGE_DIR/bin/mayhem$BIN_EXT"
  [[ -x "$RELEASE_SIGNER_BIN" && ! -L "$RELEASE_SIGNER_BIN" ]] ||
    die "fresh staged release signer must be an executable regular non-symlink file"
  TRUSTED_RELEASE_KEY="$TMP_DIR/trusted-release-key.json"
  snapshot_canonical_release_key \
    "$RELEASE_KEYS_DIR/$RELEASE_KEY_ID.json" \
    "$TRUSTED_RELEASE_KEY" \
    "$RELEASE_KEY_ID" \
    "$RELEASE_CREATED_AT"
  SIGNER_KEYS_DIR="$TMP_DIR/signer-keys"
  mkdir "$SIGNER_KEYS_DIR"
  SIGNATURE="$PUBLICATION_DIR/$ARCHIVE_BASENAME.manifest.json.sig"
  rm -f "$SIGNATURE"
  log "signing release manifest"
  "$RELEASE_SIGNER_BIN" release-sign \
    --manifest-path "$PUBLICATION_DIR/$ARCHIVE_BASENAME.manifest.json" \
    --signature-output "$SIGNATURE" \
    --keys-dir "$SIGNER_KEYS_DIR" \
    --key-id "$RELEASE_KEY_ID" \
    --seed-file "$RELEASE_SEED_FILE" \
    --force
  normalize_release_signature_output "$SIGNATURE" "$RELEASE_EPOCH"
  verify_release_signature_output \
    "$PUBLICATION_DIR/$ARCHIVE_BASENAME.manifest.json" \
    "$SIGNATURE" \
    "$TRUSTED_RELEASE_KEY" \
    "$RELEASE_KEY_ID"
  RELEASE_KEY_OUTPUT="$PUBLICATION_DIR/$ARCHIVE_BASENAME.release-key.json"
  publish_release_key_record \
    "$SIGNATURE" \
    "$TRUSTED_RELEASE_KEY" \
    "$RELEASE_KEY_OUTPUT" \
    "$RELEASE_EPOCH"
else
  rm -f "$PUBLICATION_DIR/$ARCHIVE_BASENAME.manifest.json.sig"
  rm -f "$PUBLICATION_DIR/$ARCHIVE_BASENAME.release-key.json"
fi

PUBLICATION_FILES=(
  "$(basename "$ARCHIVE")"
  "$(basename "$ARCHIVE").sha256"
  "$ARCHIVE_BASENAME.SHA256SUMS"
  "$ARCHIVE_BASENAME.manifest.json"
  "$(managed_verifier_executable_name "$VERSION" "$TARGET")"
  "$(managed_verifier_manifest_name "$VERSION" "$TARGET")"
)
if [[ "$SIGNED_RELEASE" == "1" ]]; then
  PUBLICATION_FILES+=(
    "$ARCHIVE_BASENAME.manifest.json.sig"
    "$ARCHIVE_BASENAME.release-key.json"
  )
fi
publish_verified_release_outputs "$PUBLICATION_DIR" "$OUT_DIR" "${PUBLICATION_FILES[@]}"
if [[ "$SIGNED_RELEASE" != "1" ]]; then
  rm -f \
    "$OUT_DIR/$ARCHIVE_BASENAME.manifest.json.sig" \
    "$OUT_DIR/$ARCHIVE_BASENAME.release-key.json"
fi
ARCHIVE="$OUT_DIR/$(basename "$ARCHIVE")"
if [[ "$SIGNED_RELEASE" == "1" ]]; then
  SIGNATURE="$OUT_DIR/$ARCHIVE_BASENAME.manifest.json.sig"
  RELEASE_KEY_OUTPUT="$OUT_DIR/$ARCHIVE_BASENAME.release-key.json"
fi

log "wrote $ARCHIVE"
printf 'Archive SHA-256: %s\n' "$ARCHIVE_HASH"
printf 'Checksum sidecar: %s\n' "$ARCHIVE.sha256"
if [[ -n "${SIGNATURE:-}" ]]; then
  printf 'Manifest signature: %s\n' "$SIGNATURE"
  printf 'Release public key: %s\n' "$RELEASE_KEY_OUTPUT"
fi
