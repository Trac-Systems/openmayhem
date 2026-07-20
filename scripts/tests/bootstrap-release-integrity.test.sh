#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MAYHEM_BIN="${MAYHEM_TEST_MAYHEM_BIN:-$ROOT_DIR/target/debug/mayhem}"
VERSION="${MAYHEM_TEST_VERSION:-0.2.24}"
SOURCE_GIT_SHA="0123456789abcdef0123456789abcdef01234567"

fail() {
  printf 'bootstrap-release-integrity.test: %s\n' "$*" >&2
  exit 1
}

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *) fail "test requires a Unix release target" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch="aarch64" ;;
  x86_64 | amd64) arch="x86_64" ;;
  *) fail "unsupported test architecture: $(uname -m)" ;;
esac
TARGET="$arch-$os"
BASE="mayhem-$VERSION-$TARGET"

[[ -x "$MAYHEM_BIN" ]] || fail "missing test mayhem binary: $MAYHEM_BIN"
[[ "$("$MAYHEM_BIN" --version)" == "mayhem $VERSION" ]] ||
  fail "test mayhem binary must be rebuilt at $VERSION"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-bootstrap-integrity.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
release_root="$tmp/$BASE"
manifest="$tmp/$BASE.manifest.json"
signature="$tmp/$BASE.manifest.json.sig"
release_key="$tmp/$BASE.release-key.json"
archive="$tmp/$BASE.tar.gz"
rollback_root="$tmp/rollback/$BASE"
rollback_manifest="$tmp/$BASE.rollback.manifest.json"
rollback_signature="$tmp/$BASE.rollback.manifest.json.sig"
rollback_archive="$tmp/$BASE.rollback.tar.gz"
rollback_candidate_marker="$tmp/rollback-candidate-executed"

TEST_RELEASE_ROOT="$release_root" \
  TEST_MANIFEST="$manifest" \
  TEST_SIGNATURE="$signature" \
  TEST_RELEASE_KEY="$release_key" \
  TEST_MAYHEM_BIN="$MAYHEM_BIN" \
  TEST_VERSION="$VERSION" \
  TEST_TARGET="$TARGET" \
  TEST_SOURCE_GIT_SHA="$SOURCE_GIT_SHA" \
  TEST_ROLLBACK_ROOT="$rollback_root" \
  TEST_ROLLBACK_MANIFEST="$rollback_manifest" \
  TEST_ROLLBACK_SIGNATURE="$rollback_signature" \
  TEST_ROLLBACK_MARKER="$rollback_candidate_marker" \
  node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(process.env.TEST_RELEASE_ROOT);
const version = process.env.TEST_VERSION;
const target = process.env.TEST_TARGET;
const sourceGitSha = process.env.TEST_SOURCE_GIT_SHA;
const extension = target.includes('windows') ? '.exe' : '';
const bins = [
  'mayhem',
  'mayhem-gateway',
  'mayhem-attestation-verifier',
  'mayhem-pay',
  'mayhemd',
  'mayhem-enclave',
  'mayhem-paygate',
];
const sha256 = (file) =>
  crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const write = (relative, contents, mode = 0o644) => {
  const output = path.join(root, relative);
  fs.mkdirSync(path.dirname(output), { recursive: true });
  fs.writeFileSync(output, contents, { mode });
};

fs.mkdirSync(path.join(root, 'bin'), { recursive: true });
for (const bin of bins) {
  const output = path.join(root, 'bin', `${bin}${extension}`);
  if (bin === 'mayhem') {
    fs.copyFileSync(process.env.TEST_MAYHEM_BIN, output);
  } else {
    fs.writeFileSync(output, '#!/bin/sh\nexit 0\n');
  }
  fs.chmodSync(output, 0o755);
}
write('README.md', 'signed bootstrap fixture\n');
write('share/mayhem/RULES.md', 'signed bootstrap fixture rules\n');
write(
  'share/mayhem/intercom/contract/release.json',
  `${JSON.stringify({ schema: 1, release_version: version })}\n`,
);
write(
  'share/mayhem/intercom/contract/contract.js',
  'export const contractVersion = 1;\n',
);
write('share/mayhem/intercom/src/main.js', 'export const release = true;\n');

const assets = [];
const visit = (directory, parent = []) => {
  const entries = fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
  for (const entry of entries) {
    const parts = [...parent, entry.name];
    const relative = parts.join('/');
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      visit(absolute, parts);
    } else if (entry.isFile()) {
      assets.push({ path: relative, sha256: sha256(absolute) });
    } else {
      throw new Error(`unexpected fixture entry: ${relative}`);
    }
  }
};
visit(root);
assets.sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
const binaries = bins.map((bin) => {
  const name = `${bin}${extension}`;
  const binaryPath = `bin/${name}`;
  return {
    name,
    path: binaryPath,
    sha256: assets.find((asset) => asset.path === binaryPath).sha256,
  };
});
const intercomAssets = assets
  .filter((asset) => asset.path.startsWith('share/mayhem/intercom/'))
  .map((asset) => ({ ...asset }));
const manifest = {
  schema: 1,
  name: 'mayhem',
  version,
  target,
  built_at_utc: '2026-07-20T00:00:00Z',
  source_git_sha: sourceGitSha,
  binaries,
  assets,
  intercom: {
    schema: 1,
    release_version: version,
    contract_version: 1,
    contract_code_sha256: sha256(
      path.join(root, 'share/mayhem/intercom/contract/contract.js'),
    ),
    assets: intercomAssets,
  },
};
const manifestBytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
fs.writeFileSync(path.join(root, 'manifest.json'), manifestBytes);
fs.writeFileSync(process.env.TEST_MANIFEST, manifestBytes);
const checksums = [
  ...assets,
  { path: 'manifest.json', sha256: sha256(path.join(root, 'manifest.json')) },
].sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
fs.writeFileSync(
  path.join(root, 'SHA256SUMS'),
  checksums.map((entry) => `${entry.sha256}  ${entry.path}\n`).join(''),
);

const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');
const publicDer = publicKey.export({ format: 'der', type: 'spki' });
const publicHex = publicDer.subarray(publicDer.length - 32).toString('hex');
const keyId = 'bootstrap-integrity-test';
const sign = (candidate, output) => {
  const bytes = Buffer.from(`${JSON.stringify(candidate, null, 2)}\n`);
  const detached = {
    schema_version: 1,
    alg: 'ed25519',
    signed_path: `mayhem-${candidate.version}-${candidate.target}.manifest.json`,
    key_id: keyId,
    public_key: publicHex,
    sha256: crypto.createHash('sha256').update(bytes).digest('hex'),
    sig: crypto.sign(
      null,
      Buffer.concat([
        Buffer.from('mayhem.release-manifest.v1\n', 'ascii'),
        bytes,
      ]),
      privateKey,
    ).toString('hex'),
  };
  fs.writeFileSync(output, `${JSON.stringify(detached)}\n`);
  return bytes;
};
sign(manifest, process.env.TEST_SIGNATURE);
fs.writeFileSync(
  process.env.TEST_RELEASE_KEY,
  `${JSON.stringify({
    key_id: keyId,
    alg: 'ed25519',
    public_key: publicHex,
    status: 'active',
    created_at: '2026-07-20T00:00:00Z',
  })}\n`,
);

const wrongTarget = {
  ...manifest,
  target: target.startsWith('aarch64-') ?
    target.replace('aarch64-', 'x86_64-') :
    target.replace('x86_64-', 'aarch64-'),
};
const wrongTargetBytes = sign(
  wrongTarget,
  `${process.env.TEST_SIGNATURE}.wrong-target`,
);
fs.writeFileSync(`${process.env.TEST_MANIFEST}.wrong-target`, wrongTargetBytes);

const rollbackRoot = path.resolve(process.env.TEST_ROLLBACK_ROOT);
fs.mkdirSync(path.dirname(rollbackRoot), { recursive: true });
fs.cpSync(root, rollbackRoot, {
  recursive: true,
  errorOnExist: true,
  force: false,
  dereference: false,
});
const bootstrapPath = `bin/mayhem${extension}`;
const rollbackBootstrap = path.join(rollbackRoot, bootstrapPath);
fs.writeFileSync(
  rollbackBootstrap,
  '#!/bin/sh\n' +
    `printf 'candidate executed\\n' > ${JSON.stringify(process.env.TEST_ROLLBACK_MARKER)}\n` +
    'exit 97\n',
  { mode: 0o755 },
);
const rollbackManifest = JSON.parse(JSON.stringify(manifest));
const rollbackBootstrapSha = sha256(rollbackBootstrap);
rollbackManifest.assets.find((asset) => asset.path === bootstrapPath).sha256 =
  rollbackBootstrapSha;
rollbackManifest.binaries.find((binary) => binary.path === bootstrapPath).sha256 =
  rollbackBootstrapSha;
const rollbackBytes = sign(rollbackManifest, process.env.TEST_ROLLBACK_SIGNATURE);
fs.writeFileSync(process.env.TEST_ROLLBACK_MANIFEST, rollbackBytes);
fs.writeFileSync(path.join(rollbackRoot, 'manifest.json'), rollbackBytes);
const rollbackChecksums = [
  ...rollbackManifest.assets,
  {
    path: 'manifest.json',
    sha256: sha256(path.join(rollbackRoot, 'manifest.json')),
  },
].sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
fs.writeFileSync(
  path.join(rollbackRoot, 'SHA256SUMS'),
  rollbackChecksums.map((entry) => `${entry.sha256}  ${entry.path}\n`).join(''),
);
NODE

COPYFILE_DISABLE=1 tar -czf "$archive" -C "$tmp" "$BASE"
COPYFILE_DISABLE=1 tar -czf "$rollback_archive" -C "$tmp/rollback" "$BASE"

run_installer() {
  local case_root="$1"
  shift
  local fake_bin="$case_root/fake-bin"
  local real_tar tar_marker mutate_archive mutate_manifest mutate_signature mutate_key

  mkdir -p "$case_root/user" "$fake_bin"
  cat >"$fake_bin/npm" <<EOF
#!/bin/sh
printf 'npm invoked\n' >>"$case_root/npm.log"
exit 97
EOF
  chmod +x "$fake_bin/npm"
  tar_marker="${TEST_TAR_MARKER:-}"
  mutate_archive="${TEST_MUTATE_ARCHIVE:-}"
  mutate_manifest="${TEST_MUTATE_MANIFEST:-}"
  mutate_signature="${TEST_MUTATE_SIGNATURE:-}"
  mutate_key="${TEST_MUTATE_KEY:-}"
  if [[ -n "$tar_marker$mutate_archive$mutate_manifest$mutate_signature$mutate_key" ]]; then
    real_tar="$(command -v tar)"
    cat >"$fake_bin/tar" <<EOF
#!/bin/sh
set -eu
if [ -n "$tar_marker" ]; then
  printf 'tar invoked\n' >"$tar_marker"
fi
if [ -n "$mutate_archive" ]; then
  printf '\ncorrupted original archive\n' >>"$mutate_archive"
  printf '\ncorrupted original manifest\n' >>"$mutate_manifest"
  printf '\ncorrupted original signature\n' >>"$mutate_signature"
  printf '\ncorrupted original key\n' >>"$mutate_key"
fi
exec "$real_tar" "\$@"
EOF
    chmod +x "$fake_bin/tar"
  fi
  HOME="$case_root/user" \
    PATH="$fake_bin:$PATH" \
    "$ROOT_DIR/install.sh" \
      --artifact "$archive" \
      --manifest "$manifest" \
      --signature "$signature" \
      --release-key "$release_key" \
      --release-key-id bootstrap-integrity-test \
      --source-git-sha "$SOURCE_GIT_SHA" \
      --version "$VERSION" \
      --install-dir "$case_root/install/bin" \
      --skip-pear \
      --skip-opencode \
      --no-path-update \
      "$@"
}

expect_failure() {
  local name="$1"
  shift
  local case_root="$tmp/failure-$name"
  local out="$case_root/output.log"

  mkdir -p "$case_root"
  if run_installer "$case_root" "$@" >"$out" 2>&1; then
    cat "$out" >&2
    fail "installer accepted $name"
  fi
}

positive="$tmp/positive"
mkdir -p "$positive"
if ! run_installer "$positive" >"$positive/output.log" 2>&1; then
  cat "$positive/output.log" >&2
  fail "positive signed install failed"
fi
grep -F "verified and activated signed release $VERSION" "$positive/output.log" >/dev/null ||
  fail "positive signed install did not report authenticated activation"
[[ -x "$positive/install/bin/mayhem" ]] ||
  fail "positive signed install omitted mayhem"
[[ -f "$positive/install/share/mayhem/RULES.md" ]] ||
  fail "positive signed install omitted authenticated assets"
[[ -f "$positive/install/.mayhem-update/trusted-release-keys/bootstrap-integrity-test.json" ]] ||
  fail "positive signed install did not provision updater trust"
node - "$positive/install/.mayhem-update/release-floor.json" "$VERSION" <<'NODE'
const fs = require('node:fs');
const floor = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
if (floor.schema !== 1 || floor.version !== process.argv[3]) process.exit(1);
NODE
[[ ! -e "$positive/npm.log" ]] ||
  fail "artifact install invoked npm after release authentication"

source_pin="$tmp/source-pin"
mkdir -p "$source_pin"
if ! run_installer "$source_pin" --version latest >"$source_pin/output.log" 2>&1; then
  cat "$source_pin/output.log" >&2
  fail "source_git_sha-pinned latest install failed"
fi

expect_failure unpinned-latest --version latest --source-git-sha ""
expect_failure wrong-requested-version --version 0.2.25

snapshot_root="$tmp/snapshot"
mkdir -p "$snapshot_root"
snapshot_archive="$snapshot_root/$BASE.tar.gz"
snapshot_manifest="$snapshot_root/$BASE.manifest.json"
snapshot_signature="$snapshot_root/$BASE.manifest.json.sig"
snapshot_key="$snapshot_root/$BASE.release-key.json"
cp "$archive" "$snapshot_archive"
cp "$manifest" "$snapshot_manifest"
cp "$signature" "$snapshot_signature"
cp "$release_key" "$snapshot_key"
snapshot_case="$tmp/snapshot-case"
mkdir -p "$snapshot_case"
if ! TEST_MUTATE_ARCHIVE="$snapshot_archive" \
  TEST_MUTATE_MANIFEST="$snapshot_manifest" \
  TEST_MUTATE_SIGNATURE="$snapshot_signature" \
  TEST_MUTATE_KEY="$snapshot_key" \
  run_installer \
    "$snapshot_case" \
    --artifact "$snapshot_archive" \
    --manifest "$snapshot_manifest" \
    --signature "$snapshot_signature" \
    --release-key "$snapshot_key" >"$snapshot_case/output.log" 2>&1; then
  cat "$snapshot_case/output.log" >&2
  fail "signed install reopened caller-owned inputs after private snapshot"
fi
grep -F "corrupted original archive" "$snapshot_archive" >/dev/null ||
  fail "snapshot regression did not mutate the caller-owned archive"
[[ -x "$snapshot_case/install/bin/mayhem" ]] ||
  fail "snapshot install did not activate from its private release inputs"

cp "$manifest" "$tmp/tampered.manifest.json"
printf ' ' >>"$tmp/tampered.manifest.json"
expect_failure tampered-manifest --manifest "$tmp/tampered.manifest.json"

node - "$signature" "$tmp/tampered.sig" <<'NODE'
const fs = require('node:fs');
const value = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
value.sig = `${value.sig[0] === '0' ? '1' : '0'}${value.sig.slice(1)}`;
fs.writeFileSync(process.argv[3], `${JSON.stringify(value)}\n`);
NODE
expect_failure tampered-signature --signature "$tmp/tampered.sig"

node - "$release_key" "$tmp/tampered-key.json" <<'NODE'
const fs = require('node:fs');
const value = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
value.public_key = `${value.public_key[0] === '0' ? '1' : '0'}${value.public_key.slice(1)}`;
fs.writeFileSync(process.argv[3], `${JSON.stringify(value)}\n`);
NODE
expect_failure tampered-key --release-key "$tmp/tampered-key.json"

expect_failure wrong-target \
  --manifest "$manifest.wrong-target" \
  --signature "$signature.wrong-target"
expect_failure wrong-source \
  --source-git-sha "89abcdef0123456789abcdef0123456789abcdef"

extra_root="$tmp/extra/$BASE"
mkdir -p "$tmp/extra"
cp -R "$release_root" "$extra_root"
printf 'unlisted\n' >"$extra_root/share/mayhem/extra.txt"
COPYFILE_DISABLE=1 tar -czf "$tmp/extra.tar.gz" -C "$tmp/extra" "$BASE"
expect_failure extra-file --artifact "$tmp/extra.tar.gz"

missing_root="$tmp/missing/$BASE"
mkdir -p "$tmp/missing"
cp -R "$release_root" "$missing_root"
rm "$missing_root/share/mayhem/RULES.md"
COPYFILE_DISABLE=1 tar -czf "$tmp/missing.tar.gz" -C "$tmp/missing" "$BASE"
expect_failure missing-file --artifact "$tmp/missing.tar.gz"

node - "$archive" "$tmp/tampered-archive.tar.gz" <<'NODE'
const fs = require('node:fs');
const bytes = fs.readFileSync(process.argv[2]);
bytes[Math.floor(bytes.length / 2)] ^= 0x01;
fs.writeFileSync(process.argv[3], bytes);
NODE
expect_failure tampered-archive --artifact "$tmp/tampered-archive.tar.gz"

if command -v python3 >/dev/null 2>&1; then
  python3 - "$release_root" "$BASE" "$tmp" <<'PY'
import io
import pathlib
import sys
import tarfile

root = pathlib.Path(sys.argv[1])
base = sys.argv[2]
output = pathlib.Path(sys.argv[3])

def write_archive(name, hostile):
    with tarfile.open(output / f"hostile-{name}.tar.gz", "w:gz") as archive:
        archive.add(root, arcname=base, recursive=True)
        hostile(archive)

def traversal(archive):
    payload = b"must not escape\n"
    entry = tarfile.TarInfo("../mayhem-bootstrap-escape")
    entry.size = len(payload)
    entry.mode = 0o644
    archive.addfile(entry, io.BytesIO(payload))

def symlink(archive):
    entry = tarfile.TarInfo(f"{base}/share/mayhem/hostile-symlink")
    entry.type = tarfile.SYMTYPE
    entry.linkname = "../../outside"
    archive.addfile(entry)

def hardlink(archive):
    entry = tarfile.TarInfo(f"{base}/share/mayhem/hostile-hardlink")
    entry.type = tarfile.LNKTYPE
    entry.linkname = f"{base}/README.md"
    archive.addfile(entry)

def duplicate(archive):
    archive.add(root / "README.md", arcname=f"{base}/README.md", recursive=False)

write_archive("traversal", traversal)
write_archive("symlink", symlink)
write_archive("hardlink", hardlink)
write_archive("duplicate", duplicate)
PY
  for hostile in traversal symlink hardlink duplicate; do
    expect_failure "hostile-$hostile" --artifact "$tmp/hostile-$hostile.tar.gz"
  done
fi

rollback="$tmp/failure-rollback"
mkdir -p "$rollback/install/.mayhem-update"
cat >"$rollback/install/.mayhem-update/release-floor.json" <<EOF
{
  "schema": 1,
  "version": "0.2.25"
}
EOF
rollback_output="$rollback/output.log"
rollback_tar_marker="$tmp/rollback-tar-invoked"
if TEST_TAR_MARKER="$rollback_tar_marker" \
  run_installer \
    "$rollback" \
    --artifact "$rollback_archive" \
    --manifest "$rollback_manifest" \
    --signature "$rollback_signature" >"$rollback_output" 2>&1; then
  cat "$rollback_output" >&2
  fail "installer accepted a signed release below the existing floor"
fi
grep -F "below protected anti-rollback floor" "$rollback_output" >/dev/null ||
  fail "rollback rejection did not come from the bootstrap floor comparison"
[[ ! -e "$rollback_tar_marker" ]] ||
  fail "rollback rejection opened the archive before enforcing the floor"
[[ ! -e "$rollback_candidate_marker" ]] ||
  fail "rollback rejection executed bytes from the signed candidate"

if "$ROOT_DIR/install.sh" --allow-unverified --help >/dev/null 2>&1; then
  fail "removed unverified production bypass was accepted"
fi

printf 'bootstrap-release-integrity.test: ok\n'
