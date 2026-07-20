#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const workflowPath = path.join(root, '.github/workflows/release-native-tail.yml');
const readText = (sourcePath) =>
  fs.readFileSync(sourcePath, 'utf8').replace(/\r\n?/g, '\n');
const source = readText(workflowPath);
const packageCapabilitySource = readText(
  path.join(root, 'scripts/tests/release-package-capabilities.test.sh'),
);
const cargoWorkspaceSource = readText(path.join(root, 'Cargo.toml'));
const releaseKeysDir = path.join(root, 'release/keys');
const releaseKeyName = 'openmayhem-release-v1.json';
const releaseKey = JSON.parse(
  fs.readFileSync(path.join(releaseKeysDir, releaseKeyName), 'utf8'),
);

const fail = (message) => {
  throw new Error(`release-native-tail-workflow.test: ${message}`);
};
const requireMatch = (pattern, message, text = source) => {
  if (!pattern.test(text)) fail(message);
};
const requireLiteral = (literal, message, text = source) => {
  if (!text.includes(literal)) fail(message);
};
const requireAbsent = (pattern, message, text = source) => {
  if (pattern.test(text)) fail(message);
};

const releaseKeyNames = fs.readdirSync(releaseKeysDir).sort();
if (JSON.stringify(releaseKeyNames) !== JSON.stringify([releaseKeyName])) {
  fail('release/keys must contain only the canonical public key record');
}
if (JSON.stringify(Object.keys(releaseKey)) !== JSON.stringify([
  'key_id',
  'alg',
  'public_key',
  'status',
  'created_at',
]) ||
    releaseKey.key_id !== 'openmayhem-release-v1' ||
    releaseKey.alg !== 'ed25519' ||
    releaseKey.status !== 'active' ||
    !/^[0-9a-f]{64}$/.test(releaseKey.public_key) ||
    typeof releaseKey.created_at !== 'string' ||
    releaseKey.created_at.length === 0) {
  fail('canonical release public key record is invalid');
}

const inputsMatch = source.match(
  /^    inputs:\n(?<inputs>(?:(?: {6}.*)?\n)+?)(?=^permissions:)/m,
);
if (!inputsMatch) fail('workflow_dispatch inputs block is missing');
const inputNames = [...inputsMatch.groups.inputs.matchAll(/^      ([a-z0-9_]+):$/gm)]
  .map((match) => match[1]);
if (JSON.stringify(inputNames) !== JSON.stringify(['release_tag', 'source_sha'])) {
  fail('manual inputs must be exactly release_tag and source_sha');
}
requireMatch(
  /^      release_tag:\n(?:        .*\n)*        required: true\n        type: string$/m,
  'release_tag must be a required manual string input',
  inputsMatch.groups.inputs,
);
requireMatch(
  /^      source_sha:\n(?:        .*\n)*        required: true\n        type: string$/m,
  'source_sha must be a required manual string input',
  inputsMatch.groups.inputs,
);
requireMatch(
  /^permissions:\n  contents: read$/m,
  'top-level permissions must grant only contents: read',
);
requireAbsent(/contents:\s*write/, 'contents: write must never be granted');

const jobsIndex = source.indexOf('\njobs:\n');
if (jobsIndex < 0) fail('jobs block is missing');
const jobsSource = source.slice(jobsIndex + '\njobs:\n'.length);
const jobNames = [...jobsSource.matchAll(/^  ([a-z0-9_]+):$/gm)]
  .map((match) => match[1]);
if (JSON.stringify(jobNames) !== JSON.stringify(['verify_source', 'package'])) {
  fail('workflow jobs must be exactly verify_source and package');
}
const verifyMatch = source.match(
  /^  verify_source:\n(?<job>(?:(?: {4}.*)?\n)+?)(?=^  package:)/m,
);
const packageMatch = source.match(
  /^  package:\n(?<job>(?:(?: {4}.*)?\n)+)$/m,
);
if (!verifyMatch || !packageMatch) fail('release workflow job boundaries are invalid');
const verifySource = verifyMatch.groups.job;
const packageSource = packageMatch.groups.job;

requireLiteral(
  '# REQUIRED GITHUB CONFIGURATION; this file does not claim it already exists:',
  'workflow must describe required out-of-band protection without claiming it exists',
);
requireLiteral(
  '#   only as an environment secret (no repository- or organization-secret copy).',
  'signing seed must be documented as environment-only',
);
requireLiteral(
  '# - Do not configure required reviewers or self-review gates; protected-tag\n'
    + '#   provenance and the environment-held signing secret must run autonomously.',
  'autonomous release execution must be documented',
);
requireLiteral(
  '# - Select deployment branches and tags with tag rules exactly v0.2.23 and\n'
    + '#   v0.2.23-rc.* and no branch rules.',
  'environment deployment tag allowlist must be documented exactly',
);
requireLiteral(
  '# - Protect those two tag patterns with an active repository tag ruleset that\n'
    + '#   restricts tag creation, update, and deletion to release maintainers.',
  'repository release-tag protection must be documented exactly',
);

requireLiteral(
  'runs-on: ubuntu-24.04',
  'provenance must use the fixed Ubuntu 24.04 runner',
  verifySource,
);
requireAbsent(
  /\buses:|actions\/checkout|(?:^|\s)(?:bash|node|pwsh)\s+scripts\//m,
  'provenance job must not checkout or execute repository code',
  verifySource,
);
requireAbsent(
  /secrets\.|MAYHEM_RELEASE_SEED_HEX/,
  'provenance job must not access the release signing secret',
  verifySource,
);
requireLiteral(
  '^v0\\.2\\.23(-rc\\.[1-9][0-9]*)?$',
  'release_tag must allow only v0.2.23-rc.N or v0.2.23',
  verifySource,
);
requireLiteral(
  '^[0-9a-f]{40}$',
  'source_sha must use exact lowercase 40-hex validation',
  verifySource,
);
requireLiteral(
  'WORKFLOW_SHA: ${{ github.workflow_sha }}',
  'provenance must read the commit that supplied the workflow definition',
  verifySource,
);
for (const [literal, message] of [
  ['[[ "$GITHUB_REPOSITORY" == "Trac-Systems/openmayhem" ]]',
    'dispatch must run in the canonical repository'],
  ['[[ "$GITHUB_REF_TYPE" == "tag" ]]',
    'dispatch ref must be a tag'],
  ['[[ "$GITHUB_REF" == "refs/tags/$RELEASE_TAG" ]]',
    'dispatch ref must equal the release_tag input'],
  ['[[ "$GITHUB_SHA" == "$SOURCE_SHA" ]]',
    'workflow commit must equal source_sha before requesting the environment'],
  ['[[ "$WORKFLOW_SHA" == "$SOURCE_SHA" ]]',
    'workflow definition commit must equal source_sha'],
  ['[[ "$GITHUB_REF_PROTECTED" == "true" ]]',
    'dispatch tag must be protected by an active ruleset'],
  ['git init --bare "$provenance_repo"',
    'release provenance must use an isolated repository'],
  ['origin https://github.com/Trac-Systems/openmayhem.git',
    'release provenance must fetch the canonical origin directly'],
  ['"+refs/heads/main:refs/remotes/origin/main"',
    'canonical origin/main must be fetched explicitly'],
  ['"+refs/tags/$RELEASE_TAG:refs/tags/$RELEASE_TAG"',
    'release tag must be fetched explicitly and independently'],
  ['--verify "refs/tags/$RELEASE_TAG^{commit}"',
    'release tag must be peeled to a commit'],
  ['[[ "$tag_commit" == "$SOURCE_SHA" ]]',
    'peeled release tag commit must equal source_sha'],
  ['--is-ancestor "$tag_commit" refs/remotes/origin/main',
    'release commit must be on canonical origin/main history'],
  ['printf \'release_tag=%s\\n\' "$RELEASE_TAG" >>"$GITHUB_OUTPUT"',
    'verified release_tag must be emitted for downstream binding'],
  ['printf \'source_sha=%s\\n\' "$SOURCE_SHA" >>"$GITHUB_OUTPUT"',
    'verified source_sha must be emitted for downstream binding'],
]) {
  requireLiteral(literal, message, verifySource);
}
const rawSourceInputUses = source.match(/\$\{\{ inputs\.source_sha \}\}/g) ?? [];
const rawTagInputUses = source.match(/\$\{\{ inputs\.release_tag \}\}/g) ?? [];
if (rawSourceInputUses.length !== 1 || rawTagInputUses.length !== 1) {
  fail('raw dispatch inputs must be consumed only by the provenance job');
}

requireMatch(
  new RegExp(
    '^    needs: verify_source\\n'
      + '    if: >-\\n'
      + "      github\\.ref_type == 'tag' &&\\n"
      + '      github\\.sha == needs\\.verify_source\\.outputs\\.source_sha &&\\n'
      + '      github\\.workflow_sha == needs\\.verify_source\\.outputs\\.source_sha\\n'
      + '    environment:\\n'
      + '      name: release-signing\\n'
      + '    runs-on:',
    'm',
  ),
  'secret-consuming package job must depend on provenance and bind release-signing',
  packageSource,
);
requireLiteral(
  'ref: ${{ needs.verify_source.outputs.source_sha }}',
  'checkout must use only the independently verified source_sha',
  packageSource,
);
requireAbsent(
  /ref:\s*\$\{\{\s*inputs\./,
  'checkout must never use a raw workflow_dispatch input',
  packageSource,
);
requireLiteral('fetch-depth: 0', 'checkout must fetch full history', packageSource);
requireLiteral(
  'persist-credentials: false',
  'checkout credentials must not persist',
  packageSource,
);
requireLiteral(
  'actual="$(git rev-parse --verify HEAD)"',
  'workflow must resolve the exact checked-out HEAD',
  packageSource,
);
requireLiteral(
  '[[ "$actual" == "$SOURCE_SHA" ]]',
  'checked-out HEAD must equal verified source_sha',
  packageSource,
);
requireLiteral(
  '--porcelain=v1 \\\n              --untracked-files=all \\\n'
    + '              --ignore-submodules=none',
  'workflow must reject every dirty tracked, untracked, or submodule state',
  packageSource,
);

const matrixMatch = packageSource.match(
  /^      matrix:\n(?<matrix>(?:(?: {8}.*)?\n)+?)(?=^    steps:)/m,
);
if (!matrixMatch) fail('matrix block is missing');
const matrixEntries = [...matrixMatch.groups.matrix.matchAll(
  /^          - runner: (\S+)\n            target: (\S+)\n            archive_suffix: (\S+)\n            executable_suffix: (.*)$/gm,
)].map((match) => `${match[1]}/${match[2]}/${match[3]}/${match[4]}`);
const expectedMatrix = [
  "macos-15-intel/x86_64-apple-darwin/.tar.gz/''",
  'windows-11-arm/aarch64-pc-windows-msvc/.zip/.exe',
];
if (JSON.stringify(matrixEntries) !== JSON.stringify(expectedMatrix)) {
  fail(`matrix must be exactly ${expectedMatrix.join(' and ')}`);
}
const matrixKeys = [...matrixMatch.groups.matrix.matchAll(
  /^ {8}([a-zA-Z0-9_-]+):/gm,
)].map((match) => match[1]);
if (JSON.stringify(matrixKeys) !== JSON.stringify(['include'])) {
  fail('matrix must not define additional axes');
}

const expectedActions = [
  'actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5',
  'actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020',
  'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02',
];
const actionRefs = [...source.matchAll(/^\s+uses: (\S+)$/gm)]
  .map((match) => match[1]);
if (JSON.stringify(actionRefs) !== JSON.stringify(expectedActions)) {
  fail('workflow actions must be exactly the three reviewed immutable references');
}
if (actionRefs.some((ref) => !/^[^@]+@[0-9a-f]{40}$/.test(ref))) {
  fail('every action must be pinned to a full immutable commit SHA');
}

requireMatch(
  /^          node-version: 22\.22\.0$/m,
  'Node.js must be pinned to tested version 22.22.0',
  packageSource,
);
requireAbsent(
  /node-version:\s*(?:22|lts\/\*|node)$/m,
  'Node.js must not use a moving release line',
  packageSource,
);
requireMatch(
  /^\[workspace\.package\]\n(?:(?!^\[)[\s\S])*^rust-version = "1\.89"$/m,
  'workspace Rust policy must match the locked dependency floor at 1.89',
  cargoWorkspaceSource,
);
requireLiteral(
  'rustup toolchain install "1.89.0-$TARGET" --profile minimal',
  'Rust must install exact workspace-MSRV toolchain 1.89.0',
  packageSource,
);
requireLiteral(
  'rustup default "1.89.0-$TARGET"',
  'Rust default must use exact workspace-MSRV toolchain 1.89.0',
  packageSource,
);
requireAbsent(
  /rustup (?:toolchain install|default) "?(?:stable|beta|nightly)/,
  'Rust must not use a moving toolchain channel',
  packageSource,
);

requireLiteral(
  'translated="$(sysctl -n sysctl.proc_translated 2>/dev/null || printf \'0\')"',
  'Intel macOS identity must tolerate the translation key being absent',
  packageSource,
);
requireLiteral(
  '[[ "$translated" == "0" ]]',
  'Intel macOS identity must require sysctl.proc_translated=0',
  packageSource,
);
requireLiteral(
  '[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()',
  'Windows identity must inspect OSArchitecture',
  packageSource,
);
requireLiteral(
  '[[ "$os_arch" == "Arm64" ]]',
  'Windows identity must require OSArchitecture Arm64',
  packageSource,
);
requireLiteral(
  '[[ "${VSCMD_ARG_TGT_ARCH,,}" == "arm64" ]]',
  'Windows identity must require the VS ARM64 target toolchain',
  packageSource,
);
requireLiteral(
  '-Value "CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=$linker"',
  'Windows ARM packaging must pin Cargo to the discovered MSVC linker',
  packageSource,
);
requireLiteral(
  '[[ -n "${CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER:-}" ]]',
  'Windows identity must reject an unpinned Cargo ARM64 linker',
  packageSource,
);
requireLiteral(
  '[[ "$rust_host" == "$TARGET" ]]',
  'Rust host must exactly match each matrix target',
  packageSource,
);

const secretNames = [...source.matchAll(/secrets\.([A-Z0-9_]+)/g)]
  .map((match) => match[1]);
if (JSON.stringify(secretNames) !== JSON.stringify(['MAYHEM_RELEASE_SEED_HEX'])) {
  fail('MAYHEM_RELEASE_SEED_HEX must be the only referenced workflow secret');
}
requireLiteral('umask 077', 'release seed file must be created under umask 077');
requireLiteral('trap cleanup EXIT', 'release seed file must have EXIT cleanup');
requireLiteral(
  'printf \'%s\\n\' "$MAYHEM_RELEASE_SEED_HEX" >"$seed_file"',
  'protected seed must only be written to the private seed file',
);
requireLiteral(
  '--release-seed-file "$seed_file"',
  'packager must use canonical signed mode',
);
requireLiteral('--version 0.2.23', 'packager version must be exactly 0.2.23');
requireLiteral(
  '--release-key-id "$MAYHEM_RELEASE_KEY_ID"',
  'packager must use the canonical release key id',
);
requireLiteral(
  'MAYHEM_RELEASE_KEY_ID: openmayhem-release-v1',
  'workflow must bind the canonical public release key id',
);
requireLiteral(
  'trusted_key="$PWD/release/keys/$MAYHEM_RELEASE_KEY_ID.json"',
  'workflow must source release trust from the committed key record',
);
requireLiteral(
  '--release-created-at "$release_created_at"',
  'packager must retain canonical release-key metadata',
);
requireLiteral(
  '"dist/mayhem-0.2.23-$TARGET.release-key.json" \\\n            "$trusted_key"',
  'published key record must exactly match the committed trust anchor',
);
requireAbsent(
  /--unsigned-layout|--skip-build/,
  'native signed workflow must not weaken fresh signed packaging',
);

const testIndex = source.indexOf('bash scripts/tests/release-package-capabilities.test.sh');
const packageIndex = source.indexOf('bash scripts/package-release.sh');
if (testIndex < 0 || packageIndex < 0 || testIndex > packageIndex) {
  fail('focused native package verifier tests must run before packaging');
}
if (!packageCapabilitySource.includes('command -v pwsh.exe') ||
    !packageCapabilitySource.includes('[System.IO.Compression.ZipFile]::OpenRead')) {
  fail('Windows native package capability test must inspect .NET ZipArchive output');
}

requireLiteral(
  '[[ -d dist && ! -L dist ]]',
  'inventory must reject a symlinked or missing output root',
);
requireLiteral(
  '[[ -f "dist/$name" && ! -L "dist/$name" && -s "dist/$name" ]]',
  'every expected upload must be a nonempty regular non-symlink file',
);
requireLiteral(
  'find dist -mindepth 1 -maxdepth 1 -print',
  'inventory must enumerate every top-level entry, including unexpected directories',
);
requireAbsent(
  /find dist[^\n]*-type f/,
  'inventory enumeration must not ignore unexpected non-file entries',
);
requireLiteral(
  'cmp "$expected_inventory" "$actual_inventory"',
  'actual output entries must exactly equal the expected inventory',
);
const expectedInventoryMatch = packageSource.match(
  /^          expected=\(\n(?<entries>(?:            .+\n)+?)^          \)$/m,
);
if (!expectedInventoryMatch) fail('expected release inventory array is missing');
const expectedInventoryEntries = expectedInventoryMatch.groups.entries.trim()
  .split('\n')
  .map((line) => line.trim());
const requiredInventoryEntries = [
  '"$archive"',
  '"$archive.sha256"',
  '"$base.SHA256SUMS"',
  '"$base.manifest.json"',
  '"$base.manifest.json.sig"',
  '"$base.release-key.json"',
  '"$verifier"',
  '"$verifier_base.manifest.json"',
];
if (JSON.stringify(expectedInventoryEntries) !==
    JSON.stringify(requiredInventoryEntries)) {
  fail('validated release inventory must be exactly the eight uploaded files');
}

const uploadMatch = packageSource.match(
  /^      - name: Upload native release artifacts\n(?:(?: {8}.*)?\n)*?^          path: \|\n(?<paths>(?:            .+\n)+?)^          if-no-files-found: error$/m,
);
if (!uploadMatch) fail('artifact upload step or exact path block is missing');
const uploadPaths = uploadMatch.groups.paths.trim().split('\n')
  .map((line) => line.trim());
const expectedUploadPaths = [
  'dist/mayhem-0.2.23-${{ matrix.target }}${{ matrix.archive_suffix }}',
  'dist/mayhem-0.2.23-${{ matrix.target }}${{ matrix.archive_suffix }}.sha256',
  'dist/mayhem-0.2.23-${{ matrix.target }}.SHA256SUMS',
  'dist/mayhem-0.2.23-${{ matrix.target }}.manifest.json',
  'dist/mayhem-0.2.23-${{ matrix.target }}.manifest.json.sig',
  'dist/mayhem-0.2.23-${{ matrix.target }}.release-key.json',
  'dist/mayhem-attestation-verifier-0.2.23-${{ matrix.target }}'
    + '${{ matrix.executable_suffix }}',
  'dist/mayhem-attestation-verifier-0.2.23-${{ matrix.target }}.manifest.json',
];
if (JSON.stringify(uploadPaths) !== JSON.stringify(expectedUploadPaths)) {
  fail('artifact upload must contain only the exact eight-file release inventory');
}
if (uploadPaths.some((uploadPath) => /[*?[\]]/.test(uploadPath))) {
  fail('artifact upload paths must not contain wildcard syntax');
}
requireAbsent(
  /gh\s+release|actions\/create-release|softprops\/action-gh-release/i,
  'workflow must not publish a GitHub Release',
);

process.stdout.write('release-native-tail-workflow.test: ok\n');
