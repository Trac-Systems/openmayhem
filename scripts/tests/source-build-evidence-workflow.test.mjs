import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const workflow = fs.readFileSync(
  path.join(root, '.github', 'workflows', 'source-build-evidence.yml'),
  'utf8',
);

function fail(message) {
  throw new Error(`source-build-evidence-workflow.test: ${message}`);
}

function requireText(text, message) {
  if (!workflow.includes(text)) fail(message);
}

requireText('workflow_dispatch:', 'workflow is not manually dispatchable');
requireText('permissions:\n  contents: read', 'workflow permissions are not read-only');
requireText('max-parallel: 6', 'six native jobs are not allowed to run in parallel');
requireText('git status --porcelain=v1 --untracked-files=all --ignore-submodules=none',
  'clean-source verification is missing');
requireText('[[ "$GITHUB_SHA" == "$SOURCE_SHA" ]]', 'dispatch is not bound to source_sha');
requireText('[[ "$WORKFLOW_SHA" == "$SOURCE_SHA" ]]', 'workflow bytes are not bound to source_sha');
requireText('cargo "${args[@]}"', 'native build command is missing');
requireText('build --release --workspace --bins --locked --target "$TARGET"',
  'native build is not fresh, locked, release, workspace-wide, and target-bound');
requireText("identity.releaseVersion !== '0.2.32' || identity.contractVersion !== 14",
  'release and contract identity gate is stale');

const entries = [...workflow.matchAll(
  /^          - runner: (?<runner>\S+)\n            target: (?<target>\S+)$/gm,
)].map(({ groups }) => `${groups.runner}/${groups.target}`);
const expected = [
  'macos-15/aarch64-apple-darwin',
  'macos-15-intel/x86_64-apple-darwin',
  'ubuntu-24.04-arm/aarch64-unknown-linux-gnu',
  'ubuntu-24.04/x86_64-unknown-linux-gnu',
  'windows-11-arm/aarch64-pc-windows-msvc',
  'windows-2025/x86_64-pc-windows-msvc',
];
if (JSON.stringify(entries) !== JSON.stringify(expected)) {
  fail(`native matrix must be exactly ${expected.join(', ')}`);
}

for (const [pattern, message] of [
  [/\bsecrets\s*:/i, 'workflow must not consume secrets'],
  [/\benvironment\s*:/i, 'workflow must not enter a deployment/signing environment'],
  [/actions\/cache|cache:/i, 'workflow must not reuse build caches'],
  [/upload-artifact/i, 'workflow must not upload executables or artifacts'],
  [/package-release\.sh/i, 'workflow must not package or sign executables'],
  [/gh\s+release|create-release|git\s+push/i, 'workflow must not publish or mutate repository refs'],
  [/\bdeploy(?:ment)?\b/i, 'workflow must not deploy'],
]) {
  if (pattern.test(workflow)) fail(message);
}

process.stdout.write('source-build-evidence-workflow.test: ok\n');
