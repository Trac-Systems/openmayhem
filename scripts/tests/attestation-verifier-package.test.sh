#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export MAYHEM_PACKAGE_RELEASE_SOURCE_ONLY=1
# shellcheck source=../package-release.sh
source "$ROOT_DIR/scripts/package-release.sh"
unset SOURCE_DATE_EPOCH

fail() {
  printf 'attestation-verifier-package.test: %s\n' "$*" >&2
  exit 1
}

expect_failure() {
  local message="$1"
  shift

  if ("$@") >/dev/null 2>&1; then
    fail "$message"
  fi
}

VALID_IDENTITY='{"schema_version":1,"verifier_id":"mayhem-attestation-verifier","version":1,"profiles":{"amd_sev_snp_vcek_v1":[1],"intel_tdx_dcap_v1":[1],"nvidia_nras_composite_v1":[1],"nvidia_nvtrust_offline_composite_v1":[1]},"max_input_bytes":8388608,"public_trust_source":"authenticated_admin_policy_input"}'
SOURCE_SHA="0123456789abcdef0123456789abcdef01234567"
RELEASE_EPOCH=1700000000
EXPECTED_BUILT_AT="2023-11-14T22:13:20Z"
ORIGINAL_UMASK="$(umask)"

write_identity_emitter() {
  local output="$1"
  local identity="${2:-$VALID_IDENTITY}"

  {
    printf '%s\n' '#!/bin/sh'
    printf '%s\n' 'if [ "$#" -eq 1 ] && [ "$1" = "--identity" ]; then'
    printf "  printf '%%s\\\\n' '%s'\n" "$identity"
    printf '%s\n' '  exit 0'
    printf '%s\n' 'fi'
    printf '%s\n' 'exit 1'
  } >"$output"
  chmod 0755 "$output"
}

write_release_binary_fixtures() {
  local release_dir="$1"
  local target="$2"
  local extension="$3"
  local bin

  mkdir -p "$release_dir"
  for bin in "${BINS[@]}"; do
    if [[ "$bin" == "$MANAGED_VERIFIER_ID" ]]; then
      write_identity_emitter "$release_dir/$bin$extension"
    else
      printf '#!/bin/sh\n# %s:%s\nexit 0\n' "$target" "$bin" \
        >"$release_dir/$bin$extension"
      chmod 0755 "$release_dir/$bin$extension"
    fi
  done
}

write_intercom_metadata_fixture() {
  local output="$1"

  printf '%s\n' \
    '{"schema":1,"release_version":"0.2.23","contract_version":1,"contract_code_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","assets":[]}' \
    >"$output"
}

assert_layout() {
  local stage="$1"
  local output="$2"
  local target="$3"
  local extension="$4"
  local executable_name="$5"
  local managed_manifest_name="$6"
  local archive_basename="mayhem-0.2.23-$target"
  local archive_name="$archive_basename.tar.gz"

  [[ "$target" == *-windows-* ]] && archive_name="$archive_basename.zip"
  RELEASE_STAGE_ROOT="$stage" \
    RELEASE_OUTPUT_ROOT="$output" \
    RELEASE_TARGET="$target" \
    RELEASE_EXTENSION="$extension" \
    RELEASE_SOURCE_GIT_SHA="$SOURCE_SHA" \
    RELEASE_TIMESTAMP_EPOCH="$RELEASE_EPOCH" \
    EXPECTED_BUILT_AT="$EXPECTED_BUILT_AT" \
    MANAGED_EXECUTABLE_NAME="$executable_name" \
    MANAGED_MANIFEST_NAME="$managed_manifest_name" \
    RELEASE_ARCHIVE_NAME="$archive_name" \
    RELEASE_ARCHIVE_BASENAME="$archive_basename" \
    RELEASE_BIN_NAMES="$(printf '%s\n' "${BINS[@]}")" \
    node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const stage = path.resolve(process.env.RELEASE_STAGE_ROOT);
const output = path.resolve(process.env.RELEASE_OUTPUT_ROOT);
const target = process.env.RELEASE_TARGET;
const extension = process.env.RELEASE_EXTENSION;
const sourceGitSha = process.env.RELEASE_SOURCE_GIT_SHA;
const epoch = Number(process.env.RELEASE_TIMESTAMP_EPOCH);
const expectedBuiltAt = process.env.EXPECTED_BUILT_AT;
const executableName = process.env.MANAGED_EXECUTABLE_NAME;
const managedManifestName = process.env.MANAGED_MANIFEST_NAME;
const archiveName = process.env.RELEASE_ARCHIVE_NAME;
const archiveBasename = process.env.RELEASE_ARCHIVE_BASENAME;
const binaryNames = process.env.RELEASE_BIN_NAMES.split('\n').filter(Boolean);
const executablePaths = new Set(
  binaryNames.map((name) => `bin/${name}${extension}`)
);
executablePaths.add(executableName);
const sha256 = (filePath) => crypto.createHash('sha256')
  .update(fs.readFileSync(filePath))
  .digest('hex');
const exactKeys = (value, expected) =>
  JSON.stringify(Object.keys(value)) === JSON.stringify(expected);

const genericManifestPath = path.join(stage, 'manifest.json');
const genericManifest = JSON.parse(fs.readFileSync(genericManifestPath, 'utf8'));
if (genericManifest.schema !== 1 ||
    genericManifest.name !== 'mayhem' ||
    genericManifest.version !== '0.2.23' ||
    genericManifest.target !== target ||
    genericManifest.built_at_utc !== expectedBuiltAt ||
    genericManifest.source_git_sha !== sourceGitSha ||
    !/^[0-9a-f]{40}$/.test(genericManifest.source_git_sha)) {
  throw new Error(`generic release identity is invalid for ${target}`);
}

const managedExecutablePath = path.join(stage, executableName);
const managedManifestPath = path.join(stage, managedManifestName);
const managedManifestBytes = fs.readFileSync(managedManifestPath, 'utf8');
const managedManifest = JSON.parse(managedManifestBytes);
const expectedProfiles = {
  amd_sev_snp_vcek_v1: [1],
  intel_tdx_dcap_v1: [1],
  nvidia_nras_composite_v1: [1],
  nvidia_nvtrust_offline_composite_v1: [1],
};
if (!exactKeys(managedManifest, [
  'schema_version',
  'target',
  'verifier_id',
  'version',
  'executable_sha256',
  'profiles',
]) ||
    managedManifest.schema_version !== 1 ||
    managedManifest.target !== target ||
    managedManifest.verifier_id !== 'mayhem-attestation-verifier' ||
    managedManifest.version !== 1 ||
    managedManifest.executable_sha256 !== sha256(managedExecutablePath) ||
    JSON.stringify(managedManifest.profiles) !== JSON.stringify(expectedProfiles)) {
  throw new Error(`managed verifier gateway schema is invalid for ${target}`);
}
if (!managedManifestBytes.endsWith('\n') ||
    managedManifestBytes.includes(stage) ||
    managedManifestBytes.includes(output) ||
    Object.prototype.hasOwnProperty.call(managedManifest, 'source_git_sha')) {
  throw new Error(`managed verifier manifest leaks noncanonical data for ${target}`);
}
for (const name of [executableName, managedManifestName]) {
  if (!/^[A-Za-z0-9._-]+$/.test(name) || name.includes('/')) {
    throw new Error(`managed verifier publication name is unsafe for ${target}: ${name}`);
  }
}

const assetByPath = new Map(genericManifest.assets.map((asset) => [asset.path, asset]));
for (const [relative, absolute] of [
  [executableName, managedExecutablePath],
  [managedManifestName, managedManifestPath],
]) {
  if (assetByPath.get(relative)?.sha256 !== sha256(absolute)) {
    throw new Error(`signed inventory omits managed verifier output ${relative}`);
  }
}
const verifierBinary = path.join(stage, 'bin', `mayhem-attestation-verifier${extension}`);
if (!fs.readFileSync(verifierBinary).equals(fs.readFileSync(managedExecutablePath))) {
  throw new Error(`detached verifier differs from the staged verifier for ${target}`);
}

const checksumEntries = new Map(
  fs.readFileSync(path.join(stage, 'SHA256SUMS'), 'utf8')
    .trimEnd()
    .split('\n')
    .map((line) => {
      const match = /^([0-9a-f]{64})  (.+)$/.exec(line);
      if (!match) throw new Error(`invalid SHA256SUMS line: ${line}`);
      return [match[2], match[1]];
    })
);
const checksumPaths = [...checksumEntries.keys()];
if (JSON.stringify(checksumPaths) !== JSON.stringify([...checksumPaths].sort())) {
  throw new Error(`SHA256SUMS is not path sorted for ${target}`);
}
for (const [relative, expected] of checksumEntries) {
  if (sha256(path.join(stage, relative)) !== expected) {
    throw new Error(`SHA256SUMS mismatch for ${target}: ${relative}`);
  }
}
for (const relative of [executableName, managedManifestName]) {
  if (checksumEntries.get(relative) !== sha256(path.join(stage, relative))) {
    throw new Error(`SHA256SUMS omits managed verifier output ${relative}`);
  }
}

const visit = (directory, parent = []) => {
  const relative = parent.join('/');
  const directoryStat = fs.lstatSync(directory);
  if (!directoryStat.isDirectory() ||
      (directoryStat.mode & 0o777) !== 0o755 ||
      Math.trunc(directoryStat.mtimeMs / 1000) !== epoch) {
    throw new Error(`release directory metadata is not normalized: ${relative || '.'}`);
  }
  const entries = fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    const parts = [...parent, entry.name];
    const childRelative = parts.join('/');
    const child = path.join(directory, entry.name);
    const stat = fs.lstatSync(child);
    if (stat.isSymbolicLink()) throw new Error(`normalized stage contains symlink: ${childRelative}`);
    if (stat.isDirectory()) {
      visit(child, parts);
      continue;
    }
    const expectedMode = executablePaths.has(childRelative) ? 0o755 : 0o644;
    if (!stat.isFile() ||
        (stat.mode & 0o777) !== expectedMode ||
        Math.trunc(stat.mtimeMs / 1000) !== epoch) {
      throw new Error(`release file metadata is not normalized: ${childRelative}`);
    }
  }
};
visit(stage);

const expectedOutputNames = [
  archiveName,
  `${archiveName}.sha256`,
  `${archiveBasename}.SHA256SUMS`,
  `${archiveBasename}.manifest.json`,
  executableName,
  managedManifestName,
].sort();
const outputNames = fs.readdirSync(output).sort();
if (JSON.stringify(outputNames) !== JSON.stringify(expectedOutputNames)) {
  throw new Error(`complete release output inventory is wrong for ${target}`);
}
for (const name of outputNames) {
  const stat = fs.lstatSync(path.join(output, name));
  const expectedMode = name === executableName ? 0o755 : 0o644;
  if (!stat.isFile() ||
      stat.isSymbolicLink() ||
      (stat.mode & 0o777) !== expectedMode ||
      Math.trunc(stat.mtimeMs / 1000) !== epoch) {
    throw new Error(`published output metadata is not normalized for ${target}: ${name}`);
  }
}
const archivePath = path.join(output, archiveName);
const archiveSidecar = fs.readFileSync(`${archivePath}.sha256`, 'utf8');
if (archiveSidecar !== `${sha256(archivePath)}  ${archiveName}\n`) {
  throw new Error(`archive checksum sidecar is invalid for ${target}`);
}
for (const [published, staged] of [
  [executableName, executableName],
  [managedManifestName, managedManifestName],
  [`${archiveBasename}.SHA256SUMS`, 'SHA256SUMS'],
  [`${archiveBasename}.manifest.json`, 'manifest.json'],
]) {
  if (!fs.readFileSync(path.join(output, published))
    .equals(fs.readFileSync(path.join(stage, staged)))) {
    throw new Error(`published output differs from staged inventory for ${target}: ${published}`);
  }
}
NODE
}

assert_tamper_bindings() {
  local stage="$1"
  local executable_name="$2"
  local managed_manifest_name="$3"

  RELEASE_STAGE_ROOT="$stage" \
    MANAGED_EXECUTABLE_NAME="$executable_name" \
    MANAGED_MANIFEST_NAME="$managed_manifest_name" \
    node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.RELEASE_STAGE_ROOT);
const executableName = process.env.MANAGED_EXECUTABLE_NAME;
const manifestName = process.env.MANAGED_MANIFEST_NAME;
const executablePath = path.join(root, executableName);
const manifestPath = path.join(root, manifestName);
const genericManifest = JSON.parse(fs.readFileSync(path.join(root, 'manifest.json'), 'utf8'));
const managedManifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const checksums = new Map(
  fs.readFileSync(path.join(root, 'SHA256SUMS'), 'utf8')
    .trimEnd().split('\n').map((line) => [line.slice(66), line.slice(0, 64)])
);
const sha256 = (filePath) => crypto.createHash('sha256')
  .update(fs.readFileSync(filePath)).digest('hex');
const assetHash = (name) =>
  genericManifest.assets.find(({ path: assetPath }) => assetPath === name)?.sha256;

fs.appendFileSync(executablePath, 'tampered\n');
const executableHash = sha256(executablePath);
if ([
  managedManifest.executable_sha256,
  assetHash(executableName),
  checksums.get(executableName),
].includes(executableHash)) {
  throw new Error('tampered executable escaped managed and generic digest bindings');
}
fs.appendFileSync(manifestPath, ' ');
const manifestHash = sha256(manifestPath);
if ([assetHash(manifestName), checksums.get(manifestName)].includes(manifestHash)) {
  throw new Error('tampered managed manifest escaped generic digest bindings');
}
NODE
}

verifier_count=0
for bin in "${BINS[@]}"; do
  [[ "$bin" == "$MANAGED_VERIFIER_ID" ]] &&
    verifier_count=$((verifier_count + 1))
done
[[ "$verifier_count" == "1" ]] ||
  fail "release binary inventory must contain the verifier exactly once"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-av-package-test.XXXXXX")"
trap 'rm -rf "$temp_root"' EXIT

test_provenance_guards() {
  local clean_repo="$temp_root/clean-repo"
  local tracked_repo="$temp_root/tracked-dirty-repo"
  local untracked_repo="$temp_root/untracked-dirty-repo"
  local repository candidate sha commit_epoch

  for repository in "$clean_repo" "$tracked_repo" "$untracked_repo"; do
    mkdir -p "$repository"
    git -C "$repository" init -q
    git -C "$repository" config user.name "Package Test"
    git -C "$repository" config user.email "package-test@example.invalid"
    printf 'release source\n' >"$repository/tracked.txt"
    git -C "$repository" add tracked.txt
    git -C "$repository" commit -q -m initial
  done
  sha="$(clean_source_git_sha "$clean_repo")"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] ||
    fail "clean source provenance did not return a full commit id"
  commit_epoch="$(git -C "$clean_repo" show -s --format=%ct "$sha")"
  [[ "$(resolve_release_epoch "$clean_repo" "$sha")" == "$commit_epoch" ]] ||
    fail "release epoch did not fall back to the clean commit timestamp"
  [[ "$(SOURCE_DATE_EPOCH="$RELEASE_EPOCH" \
    resolve_release_epoch "$clean_repo" "$sha")" == "$RELEASE_EPOCH" ]] ||
    fail "validated SOURCE_DATE_EPOCH did not override the commit timestamp"
  printf 'dirty\n' >>"$tracked_repo/tracked.txt"
  expect_failure "tracked dirty source was accepted" \
    clean_source_git_sha "$tracked_repo"
  printf 'untracked\n' >"$untracked_repo/untracked.txt"
  expect_failure "untracked dirty source was accepted" \
    clean_source_git_sha "$untracked_repo"

  TARGET="$(host_target)"
  UNSIGNED_LAYOUT=0
  SKIP_BUILD=1
  VERIFIER_IDENTITY_FILE=""
  expect_failure "signed release mode accepted --skip-build" \
    validate_release_mode 1
  SKIP_BUILD=0
  UNSIGNED_LAYOUT=1
  expect_failure "signed release mode accepted --unsigned-layout" \
    validate_release_mode 1
  UNSIGNED_LAYOUT=0
  expect_failure "unsigned release mode omitted explicit --unsigned-layout" \
    validate_release_mode 0
  for candidate in "${MANAGED_VERIFIER_TARGETS[@]}"; do
    if [[ "$candidate" != "$(host_target)" ]]; then
      TARGET="$candidate"
      break
    fi
  done
  expect_failure "signed release mode accepted a cross target" \
    validate_release_mode 1
  TARGET="$(host_target)"
  VERIFIER_IDENTITY_FILE="$clean_repo/tracked.txt"
  expect_failure "signed release mode accepted an identity fixture" \
    validate_release_mode 1
  VERIFIER_IDENTITY_FILE=""
}

test_symlink_binary_rejection() {
  local target="x86_64-unknown-linux-gnu"
  local release_dir="$temp_root/symlink-release"
  local stage_dir="$temp_root/symlink-stage"

  BIN_EXT=""
  write_release_binary_fixtures "$release_dir" "$target" ""
  rm "$release_dir/$MANAGED_VERIFIER_ID"
  ln -s mayhem "$release_dir/$MANAGED_VERIFIER_ID"
  expect_failure "symlink binary source was accepted" \
    stage_release_binaries "$release_dir" "$stage_dir"
}

test_identity_rejection() {
  local target=""
  local extension=""
  local release_dir="$temp_root/identity-release"
  local bad_version="$temp_root/identity-bad-version.json"
  local bad_profile="$temp_root/identity-bad-profile.json"
  local tampered="$temp_root/identity-tampered.json"
  local candidate stage

  for candidate in "${MANAGED_VERIFIER_TARGETS[@]}"; do
    if [[ "$candidate" != "$(host_target)" ]]; then
      target="$candidate"
      break
    fi
  done
  [[ -n "$target" ]] || fail "identity test could not select a cross target"
  [[ "$target" == *-windows-* ]] && extension=".exe"

  VERSION="0.2.23"
  TARGET="$target"
  BIN_EXT="$extension"
  write_release_binary_fixtures "$release_dir" "$target" "$extension"
  write_identity_emitter \
    "$temp_root/bad-version-emitter" \
    "${VALID_IDENTITY/\"version\":1/\"version\":2}"
  "$temp_root/bad-version-emitter" --identity >"$bad_version"
  write_identity_emitter \
    "$temp_root/bad-profile-emitter" \
    "${VALID_IDENTITY/\"amd_sev_snp_vcek_v1\":\[1\]/\"amd_sev_snp_vcek_v1\":[2]}"
  "$temp_root/bad-profile-emitter" --identity >"$bad_profile"
  write_identity_emitter "$temp_root/valid-emitter"
  "$temp_root/valid-emitter" --identity >"$tampered"
  printf ' ' >>"$tampered"

  stage="$temp_root/identity-stage-missing-explicit-fixture"
  stage_release_binaries "$release_dir" "$stage"
  expect_failure "cross-target layout accepted implicit verifier identity" \
    stage_managed_verifier_artifacts "$stage"

  for identity in "$bad_version" "$bad_profile" "$tampered"; do
    stage="$temp_root/identity-stage-$(basename "$identity")"
    stage_release_binaries "$release_dir" "$stage"
    expect_failure "mismatched or tampered verifier identity was accepted: $identity" \
      stage_managed_verifier_artifacts "$stage" "$identity"
  done
}

test_target() {
  local target="$1"
  local extension="$2"
  local executable_name managed_manifest_name archive_basename run
  local work stage output release_dir metadata identity_file archive_name invalid_stage

  VERSION="0.2.23"
  TARGET="$target"
  BIN_EXT="$extension"
  SOURCE_GIT_SHA="$SOURCE_SHA"
  BUILT_AT="$EXPECTED_BUILT_AT"
  executable_name="$(managed_verifier_executable_name "$VERSION" "$TARGET")"
  managed_manifest_name="$(managed_verifier_manifest_name "$VERSION" "$TARGET")"
  archive_basename="mayhem-$VERSION-$TARGET"
  archive_name="$archive_basename.tar.gz"
  [[ "$target" == *-windows-* ]] && archive_name="$archive_basename.zip"

  [[ "$executable_name" == \
    "mayhem-attestation-verifier-0.2.23-$target$extension" ]] ||
    fail "managed verifier executable name is not canonical for $target"
  [[ "$managed_manifest_name" == \
    "mayhem-attestation-verifier-0.2.23-$target.manifest.json" ]] ||
    fail "managed verifier manifest name is not canonical for $target"
  [[ "$managed_manifest_name" != *".exe.manifest.json" ]] ||
    fail "managed verifier manifest name depends on .exe for $target"

  for run in 1 2; do
    if [[ "$run" == "1" ]]; then
      umask 022
      export TZ=UTC
    else
      umask 077
      export TZ=Pacific/Honolulu
    fi
    work="$temp_root/$target-$run-work"
    stage="$work/$archive_basename"
    output="$temp_root/$target-$run-output"
    release_dir="$temp_root/$target-$run-release"
    metadata="$temp_root/$target-$run-intercom.json"
    identity_file="$temp_root/$target-$run-identity.json"
    mkdir -p "$stage/share/mayhem/runtime" "$output"
    write_release_binary_fixtures "$release_dir" "$target" "$extension"
    stage_release_binaries "$release_dir" "$stage"
    printf 'runtime fixture for %s\n' "$target" \
      >"$stage/share/mayhem/runtime/layout.txt"
    chmod 0777 "$stage/share/mayhem/runtime/layout.txt"
    write_intercom_metadata_fixture "$metadata"

    if [[ "$target" == "$(host_target)" ]]; then
      stage_managed_verifier_artifacts "$stage"
    else
      "$release_dir/$MANAGED_VERIFIER_ID$extension" --identity >"$identity_file"
      stage_managed_verifier_artifacts "$stage" "$identity_file"
    fi
    write_release_manifest "$stage" "$metadata" "$stage/manifest.json"
    write_stage_checksums "$stage"
    publish_release_stage_outputs \
      "$stage" \
      "$work" \
      "$output" \
      "$archive_basename" \
      "$target" \
      "$RELEASE_EPOCH"
    assert_layout \
      "$stage" \
      "$output" \
      "$target" \
      "$extension" \
      "$executable_name" \
      "$managed_manifest_name"
  done
  umask "$ORIGINAL_UMASK"

  cmp \
    "$temp_root/$target-1-output/$archive_name" \
    "$temp_root/$target-2-output/$archive_name" \
    >/dev/null ||
    fail "complete release archive is nondeterministic for $target"
  diff -rq \
    "$temp_root/$target-1-output" \
    "$temp_root/$target-2-output" \
    >/dev/null ||
    fail "complete release output set is nondeterministic for $target"

  invalid_stage="$temp_root/$target-invalid-source-sha"
  cp -R "$temp_root/$target-2-work/$archive_basename" "$invalid_stage"
  rm "$invalid_stage/manifest.json" "$invalid_stage/SHA256SUMS"
  SOURCE_GIT_SHA="0123456789abcdef0123456789abcdef0123456Z"
  expect_failure "generic manifest accepted malformed source_git_sha for $target" \
    write_release_manifest \
    "$invalid_stage" \
    "$temp_root/$target-2-intercom.json" \
    "$invalid_stage/manifest.json"
  SOURCE_GIT_SHA="$SOURCE_SHA"

  assert_tamper_bindings \
    "$temp_root/$target-1-work/$archive_basename" \
    "$executable_name" \
    "$managed_manifest_name"
}

test_provenance_guards
test_symlink_binary_rejection
test_identity_rejection
[[ "$(release_epoch_iso8601 "$RELEASE_EPOCH")" == "$EXPECTED_BUILT_AT" ]] ||
  fail "release timestamp is not derived deterministically from the epoch"
expect_failure "invalid SOURCE_DATE_EPOCH was accepted" validate_release_epoch "01"

target_cases=(
  "aarch64-apple-darwin|"
  "aarch64-pc-windows-msvc|.exe"
  "aarch64-unknown-linux-gnu|"
  "x86_64-apple-darwin|"
  "x86_64-pc-windows-msvc|.exe"
  "x86_64-unknown-linux-gnu|"
)
[[ "${#target_cases[@]}" == "6" ]] ||
  fail "release verifier target matrix must contain exactly six targets"
[[ "${#MANAGED_VERIFIER_TARGETS[@]}" == "6" ]] ||
  fail "packager managed verifier target matrix must contain exactly six targets"
for index in "${!target_cases[@]}"; do
  [[ "${target_cases[$index]%%|*}" == "${MANAGED_VERIFIER_TARGETS[$index]}" ]] ||
    fail "test target matrix differs from packager target matrix at index $index"
done
for target_case in "${target_cases[@]}"; do
  test_target "${target_case%%|*}" "${target_case#*|}"
done

printf 'attestation-verifier-package.test: ok\n'
