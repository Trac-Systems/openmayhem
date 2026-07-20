#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export MAYHEM_PACKAGE_RELEASE_SOURCE_ONLY=1
# shellcheck source=../package-release.sh
source "$ROOT_DIR/scripts/package-release.sh"

fail() {
  printf 'release-package-capabilities.test: %s\n' "$*" >&2
  exit 1
}

expect_failure() {
  local message="$1"
  shift

  if ("$@") >/dev/null 2>&1; then
    fail "$message"
  fi
}

intercom_eol="$(
  git -C "$ROOT_DIR" check-attr eol -- intercom/contract/contract.js |
    tr -d '\r'
)"
[[ "$intercom_eol" == *": eol: lf" ]] ||
  fail "byte-hashed Intercom release source is not pinned to LF checkouts"

node_sees_symlink() {
  node -e \
    "process.exit(require('node:fs').lstatSync(process.argv[1]).isSymbolicLink() ? 0 : 1)" \
    "$1"
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-release-capabilities.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

node - "$ROOT_DIR/intercom" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(process.argv[2]);
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const lock = JSON.parse(fs.readFileSync(path.join(root, 'package-lock.json'), 'utf8'));
const npmrc = fs.readFileSync(path.join(root, '.npmrc'), 'utf8');
if (!/^\s*install-links\s*=\s*true\s*$/m.test(npmrc)) {
  throw new Error('Intercom root npm config does not enable install-links');
}
if (manifest.dependencies?.['trac-wallet'] !== '1.0.1' ||
    manifest.overrides?.['trac-wallet'] !== '1.0.1') {
  throw new Error('Intercom root does not pin and override trac-wallet 1.0.1');
}
for (const [name, source] of [
  ['trac-msb', 'trac/msb'],
  ['trac-peer', 'trac/trac-peer'],
]) {
  const locked = lock.packages?.[`node_modules/${name}`];
  if (manifest.dependencies?.[name] !== `file:${source}` ||
      !locked ||
      locked.link === true ||
      locked.resolved !== `file:${source}`) {
    throw new Error(`${name} is not a physical root-lock file dependency`);
  }
  if (Object.keys(lock.packages).some(
    (entry) => entry === source || entry.startsWith(`${source}/`),
  )) {
    throw new Error(`${name} retains a source-owned lock subtree`);
  }
}
const wallets = Object.entries(lock.packages)
  .filter(([entry]) => entry === 'node_modules/trac-wallet' ||
    entry.endsWith('/node_modules/trac-wallet'));
if (wallets.length !== 1 ||
    wallets[0][0] !== 'node_modules/trac-wallet' ||
    wallets[0][1].version !== '1.0.1') {
  throw new Error('root lock does not contain exactly one top-level trac-wallet 1.0.1');
}
NODE

release_hydration="$(
  sed -n '/^hydrate_intercom_runtime_tree() {/,/^}/p' \
    "$ROOT_DIR/scripts/package-release.sh"
)"
[[ "$(grep -c 'npm ci' <<<"$release_hydration")" -eq 1 ]] ||
  fail "release packaging does not use exactly one root Intercom npm ci"
grep -F -- '--install-links=true' <<<"$release_hydration" >/dev/null ||
  fail "release packaging does not force physical file dependencies"
grep -F 'verify-intercom-dependency-topology.mjs' <<<"$release_hydration" >/dev/null ||
  fail "release packaging does not verify the hydrated Intercom topology"
grep -F 'materialize-local-dependencies.mjs' <<<"$release_hydration" >/dev/null ||
  fail "release packaging does not restore exact pinned runtime files after npm packing"
grep -F 'native_host="$(native_host_target)"' <<<"$release_hydration" >/dev/null ||
  fail "Intercom native dependency checks trust emulated shell architecture"
grep -F '[[ "$TARGET" == "$(native_host_target)" ]]' \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "managed verifier staging trusts emulated shell architecture"
grep -F 'TARGET="$(native_host_target)"' \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "default release target trusts emulated shell architecture"
if grep -E 'trac-(msb|peer).*(npm ci)|local_package' \
  <<<"$release_hydration" >/dev/null; then
  fail "release packaging retains separate local-package hydration"
fi

topology="$tmp/intercom-topology"
mkdir -p \
  "$topology/trac/msb" \
  "$topology/trac/trac-peer" \
  "$topology/scripts" \
  "$topology/node_modules/trac-msb" \
  "$topology/node_modules/trac-peer" \
  "$topology/node_modules/trac-wallet" \
  "$topology/node_modules/wallet-consumer"
printf 'install-links=true\n' >"$topology/.npmrc"
cp \
  "$ROOT_DIR/intercom/scripts/materialize-local-dependencies.mjs" \
  "$topology/scripts/materialize-local-dependencies.mjs"
cat >"$topology/package.json" <<'JSON'
{
  "name": "topology-root",
  "version": "1.0.0",
  "dependencies": {
    "trac-msb": "file:trac/msb",
    "trac-peer": "file:trac/trac-peer",
    "trac-wallet": "1.0.1"
  },
  "overrides": {
    "trac-wallet": "1.0.1"
  }
}
JSON
cat >"$topology/package-lock.json" <<'JSON'
{
  "name": "topology-root",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "": {
      "name": "topology-root",
      "version": "1.0.0"
    },
    "node_modules/trac-msb": {
      "name": "trac-msb",
      "version": "0.2.9",
      "resolved": "file:trac/msb"
    },
    "node_modules/trac-peer": {
      "name": "trac-peer",
      "version": "0.4.0",
      "resolved": "file:trac/trac-peer"
    },
    "node_modules/trac-wallet": {
      "name": "trac-wallet",
      "version": "1.0.1"
    },
    "node_modules/wallet-consumer": {
      "name": "wallet-consumer",
      "version": "1.0.0"
    }
  }
}
JSON
cat >"$topology/trac/msb/package.json" <<'JSON'
{"name":"trac-msb","version":"0.2.9","dependencies":{"trac-wallet":"1.0.1"}}
JSON
cat >"$topology/trac/trac-peer/package.json" <<'JSON'
{"name":"trac-peer","version":"0.4.0","dependencies":{"trac-wallet":"^0.0.43"}}
JSON
cp "$topology/trac/msb/package.json" "$topology/node_modules/trac-msb/package.json"
cp "$topology/trac/trac-peer/package.json" "$topology/node_modules/trac-peer/package.json"
for relative in migration proto rpc src whitelist; do
  mkdir -p \
    "$topology/trac/msb/$relative" \
    "$topology/node_modules/trac-msb/$relative"
done
printf 'fixture migration\n' >"$topology/trac/msb/migration/initial_balances.csv"
cp \
  "$topology/trac/msb/migration/initial_balances.csv" \
  "$topology/node_modules/trac-msb/migration/initial_balances.csv"
printf 'export {};\n' >"$topology/trac/msb/msb.mjs"
cp "$topology/trac/msb/msb.mjs" "$topology/node_modules/trac-msb/msb.mjs"
for relative in rpc src; do
  mkdir -p \
    "$topology/trac/trac-peer/$relative" \
    "$topology/node_modules/trac-peer/$relative"
done
mkdir -p \
  "$topology/trac/trac-peer/scripts" \
  "$topology/node_modules/trac-peer/scripts"
printf 'export {};\n' >"$topology/trac/trac-peer/scripts/run-peer.mjs"
cp \
  "$topology/trac/trac-peer/scripts/run-peer.mjs" \
  "$topology/node_modules/trac-peer/scripts/run-peer.mjs"
cat >"$topology/node_modules/trac-wallet/package.json" <<'JSON'
{"name":"trac-wallet","version":"1.0.1","main":"index.js"}
JSON
printf 'module.exports = {};\n' >"$topology/node_modules/trac-wallet/index.js"
cat >"$topology/node_modules/wallet-consumer/package.json" <<'JSON'
{"name":"wallet-consumer","version":"1.0.0","optionalDependencies":{"trac-wallet":"*"}}
JSON

topology_output="$(
  node "$ROOT_DIR/scripts/verify-intercom-dependency-topology.mjs" "$topology"
)"
grep -F '4 wallet resolution contexts' <<<"$topology_output" >/dev/null ||
  fail "topology verifier did not inspect every installed wallet declarer"

rm "$topology/node_modules/trac-msb/migration/initial_balances.csv"
expect_failure "topology verifier accepted an npm-omitted pinned MSB runtime file" \
  node "$ROOT_DIR/scripts/verify-intercom-dependency-topology.mjs" "$topology"
node "$topology/scripts/materialize-local-dependencies.mjs" "$topology" >/dev/null

mkdir "$topology/trac/msb/node_modules"
expect_failure "topology verifier accepted source-owned dependencies" \
  node "$ROOT_DIR/scripts/verify-intercom-dependency-topology.mjs" "$topology"
rmdir "$topology/trac/msb/node_modules"

mkdir -p "$topology/node_modules/trac-peer/node_modules"
cp -R \
  "$topology/node_modules/trac-wallet" \
  "$topology/node_modules/trac-peer/node_modules/trac-wallet"
expect_failure "topology verifier accepted a nested trac-wallet" \
  node "$ROOT_DIR/scripts/verify-intercom-dependency-topology.mjs" "$topology"
rm -rf "$topology/node_modules/trac-peer/node_modules"

mv "$topology/node_modules/trac-peer" "$topology/trac-peer-installed"
if ln -s ../trac-peer-installed "$topology/node_modules/trac-peer" 2>/dev/null; then
  if node_sees_symlink "$topology/node_modules/trac-peer"; then
    expect_failure "topology verifier accepted linked trac-peer" \
      node "$ROOT_DIR/scripts/verify-intercom-dependency-topology.mjs" "$topology"
  fi
  rm -rf "$topology/node_modules/trac-peer"
fi
mv "$topology/trac-peer-installed" "$topology/node_modules/trac-peer"

work="$tmp/work"
archive_root="mayhem-0.2.25-x86_64-pc-windows-msvc"
mkdir -p "$work/$archive_root/bin"
printf 'deterministic windows payload\n' >"$work/$archive_root/bin/mayhem.exe"
touch -t 202311142213.20 \
  "$work/$archive_root/bin/mayhem.exe" \
  "$work/$archive_root/bin" \
  "$work/$archive_root"
cat >"$tmp/files.txt" <<EOF
$archive_root/
$archive_root/bin/
$archive_root/bin/mayhem.exe
EOF

# Info-ZIP must not influence signed Windows package bytes.
zip() {
  fail "Windows packaging invoked Info-ZIP zip"
}
zipinfo() {
  fail "Windows packaging invoked Info-ZIP zipinfo"
}
tar() {
  fail "Windows packaging invoked an archive backend other than .NET ZipArchive"
}

TZ=UTC create_deterministic_windows_zip \
  "$work" \
  "$tmp/files.txt" \
  "$tmp/actual-1.txt" \
  "$tmp/release-1.zip" \
  1700000000
chmod 0600 "$work/$archive_root/bin/mayhem.exe"
TZ=Pacific/Honolulu create_deterministic_windows_zip \
  "$work" \
  "$tmp/files.txt" \
  "$tmp/actual-2.txt" \
  "$tmp/release-2.zip" \
  1700000000
cmp "$tmp/release-1.zip" "$tmp/release-2.zip" >/dev/null ||
  fail "capability-checked .NET ZIP output changed across metadata/time-zone changes"
cmp "$tmp/files.txt" "$tmp/actual-1.txt" >/dev/null ||
  fail ".NET ZIP output changed the canonical inventory"

cp "$tmp/release-1.zip" "$tmp/data-descriptor.zip"
node - "$tmp/data-descriptor.zip" <<'NODE'
const fs = require('node:fs');
const file = process.argv[2];
const bytes = fs.readFileSync(file);
const local = bytes.indexOf(Buffer.from([0x50, 0x4b, 0x03, 0x04]));
const central = bytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
if (local < 0 || central < 0) throw new Error('test ZIP headers are missing');
bytes.writeUInt16LE(bytes.readUInt16LE(local + 6) | 0x0008, local + 6);
bytes.writeUInt16LE(bytes.readUInt16LE(central + 8) | 0x0008, central + 8);
fs.writeFileSync(file, bytes);
NODE
expect_failure "ZIP capability check accepted a data-descriptor flag" \
  validate_deterministic_windows_zip \
  "$tmp/data-descriptor.zip" \
  "$tmp/files.txt"

if ln -s mayhem.exe "$work/$archive_root/bin/mayhem-link.exe" 2>/dev/null; then
  if node_sees_symlink "$work/$archive_root/bin/mayhem-link.exe"; then
    cp "$tmp/files.txt" "$tmp/symlink-files.txt"
    printf '%s\n' "$archive_root/bin/mayhem-link.exe" >>"$tmp/symlink-files.txt"
    expect_failure "deterministic ZIP writer followed a symbolic-link source" \
      create_deterministic_windows_zip \
      "$work" \
      "$tmp/symlink-files.txt" \
      "$tmp/symlink-actual.txt" \
      "$tmp/symlink.zip" \
      1700000000
  fi
  rm -rf "$work/$archive_root/bin/mayhem-link.exe"
fi

if grep -E '(^|[[:space:]])zip[[:space:]]+-|zipinfo' \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null; then
  fail "package script retains a backend-dependent Info-ZIP route"
fi

powershell_bin="$(
  command -v pwsh.exe 2>/dev/null ||
    command -v powershell.exe 2>/dev/null ||
    true
)"
if [[ -n "$powershell_bin" ]]; then
  cat >"$tmp/validate-windows-zip.ps1" <<'POWERSHELL'
param(
    [Parameter(Mandatory = $true)][string]$Archive,
    [Parameter(Mandatory = $true)][string]$ExpectedList
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$expected = @(Get-Content -LiteralPath $ExpectedList)
$zip = [System.IO.Compression.ZipFile]::OpenRead($Archive)
try {
    $actual = @($zip.Entries | ForEach-Object { $_.FullName })
    if ($actual.Count -ne $expected.Count) {
        throw "native ZIP entry count differs from the canonical inventory"
    }
    for ($index = 0; $index -lt $expected.Count; $index += 1) {
        if ($actual[$index] -cne $expected[$index]) {
            throw "native ZIP entry order differs at index $index"
        }
    }
    foreach ($entry in $zip.Entries) {
        if ($entry.FullName.Contains("\") -or
            $entry.FullName -cmatch "(^|/)\.\.?(/|$)") {
            throw "native ZIP reader observed an unsafe entry path"
        }
        $external = [BitConverter]::ToUInt32(
            [BitConverter]::GetBytes([Int32]$entry.ExternalAttributes),
            0
        )
        $unixType = ($external -shr 16) -band 0xF000
        if ($unixType -notin @(0, 0x4000, 0x8000)) {
            throw "native ZIP reader observed a link or special file"
        }
    }
} finally {
    $zip.Dispose()
}
POWERSHELL
  "$powershell_bin" \
    -NoLogo \
    -NoProfile \
    -NonInteractive \
    -File "$tmp/validate-windows-zip.ps1" \
    -Archive "$tmp/release-1.zip" \
    -ExpectedList "$tmp/files.txt"
fi

manifest_name="mayhem-0.2.25-x86_64-pc-windows-msvc.manifest.json"
manifest_path="$tmp/$manifest_name"
signature_path="$tmp/$manifest_name.sig"
key_path="$tmp/test-key.json"
TEST_MANIFEST="$manifest_path" \
  TEST_SIGNATURE="$signature_path" \
  TEST_KEY="$key_path" \
  node --input-type=module <<'NODE'
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const manifestPath = path.resolve(process.env.TEST_MANIFEST);
const manifest = {
  schema: 1,
  name: 'mayhem',
  version: '0.2.25',
  target: 'x86_64-pc-windows-msvc',
  built_at_utc: '2026-07-20T00:00:00Z',
  source_git_sha: '0123456789abcdef0123456789abcdef01234567',
  binaries: [],
  assets: [],
  intercom: {},
};
const manifestBytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
fs.writeFileSync(manifestPath, manifestBytes);
const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');
const publicDer = publicKey.export({ format: 'der', type: 'spki' });
const publicHex = publicDer.subarray(publicDer.length - 32).toString('hex');
const signature = {
  schema_version: 1,
  alg: 'ed25519',
  signed_path: path.basename(manifestPath),
  key_id: 'test-key',
  public_key: publicHex,
  sha256: crypto.createHash('sha256').update(manifestBytes).digest('hex'),
  sig: crypto.sign(
    null,
    Buffer.concat([
      Buffer.from('mayhem.release-manifest.v1\n', 'ascii'),
      manifestBytes,
    ]),
    privateKey,
  ).toString('hex'),
};
fs.writeFileSync(process.env.TEST_SIGNATURE, `${JSON.stringify(signature)}\n`);
fs.writeFileSync(
  process.env.TEST_KEY,
  `${JSON.stringify({
    key_id: 'test-key',
    alg: 'ed25519',
    public_key: publicHex,
    status: 'active',
    created_at: '2026-07-20T00:00:00Z',
  })}\n`,
);
NODE

trusted_key_path="$tmp/trusted-release-key.json"
snapshot_canonical_release_key "$key_path" "$trusted_key_path" test-key
cmp "$key_path" "$trusted_key_path" >/dev/null ||
  fail "pre-provisioned canonical release key changed while it was snapshotted"
expect_failure "canonical release key snapshot accepted the wrong expected key id" \
  snapshot_canonical_release_key \
  "$key_path" \
  "$tmp/wrong-id-snapshot.json" \
  wrong-key
expect_failure "canonical release key snapshot accepted the wrong created_at" \
  snapshot_canonical_release_key \
  "$key_path" \
  "$tmp/wrong-created-at-snapshot.json" \
  test-key \
  2026-07-21T00:00:00Z

verify_release_signature_output \
  "$manifest_path" \
  "$signature_path" \
  "$trusted_key_path" \
  test-key

mkdir "$tmp/tampered"
cp "$manifest_path" "$tmp/tampered/$manifest_name"
printf ' ' >>"$tmp/tampered/$manifest_name"
expect_failure "independent verifier accepted tampered exact manifest bytes" \
  verify_release_signature_output \
  "$tmp/tampered/$manifest_name" \
  "$signature_path" \
  "$trusted_key_path" \
  test-key

node - "$signature_path" "$tmp/tampered-signature.json" <<'NODE'
const fs = require('node:fs');
const signature = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
signature.sig = `${signature.sig[0] === '0' ? '1' : '0'}${signature.sig.slice(1)}`;
fs.writeFileSync(process.argv[3], `${JSON.stringify(signature)}\n`);
NODE
expect_failure "independent verifier trusted signer JSON without valid Ed25519 bytes" \
  verify_release_signature_output \
  "$manifest_path" \
  "$tmp/tampered-signature.json" \
  "$trusted_key_path" \
  test-key

publish_release_key_record \
  "$signature_path" \
  "$trusted_key_path" \
  "$tmp/mayhem-0.2.25-x86_64-pc-windows-msvc.release-key.json" \
  1700000000
cmp \
  "$trusted_key_path" \
  "$tmp/mayhem-0.2.25-x86_64-pc-windows-msvc.release-key.json" >/dev/null ||
  fail "published public release-key record changed bytes"

sed 's/"public_key":"[^"]*"/"public_key":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"/' \
  "$key_path" >"$tmp/wrong-key.json"
expect_failure "mismatched public release key was published" \
  publish_release_key_record \
  "$signature_path" \
  "$tmp/wrong-key.json" \
  "$tmp/wrong-output.json" \
  1700000000

uname() {
  case "$1" in
    -s) printf 'MINGW64_NT-10.0-26100\n' ;;
    -m) printf 'x86_64\n' ;;
    *) return 1 ;;
  esac
}
rustc() {
  [[ "${1:-}" == "-vV" ]] || return 1
  printf 'rustc 1.89.0\nhost: aarch64-pc-windows-msvc\n'
}
[[ "$(native_host_target)" == "aarch64-pc-windows-msvc" ]] ||
  fail "native Windows ARM detection trusted the emulated Git-Bash architecture"
unset -f uname rustc

host_target() {
  printf 'x86_64-apple-darwin\n'
}
native_host_target() {
  printf 'aarch64-apple-darwin\n'
}
TARGET="x86_64-apple-darwin"
UNSIGNED_LAYOUT=0
SKIP_BUILD=0
VERIFIER_IDENTITY_FILE=""
expect_failure "Rosetta x86_64 package counted as native signed Intel evidence" \
  validate_release_mode 1

grep -F '"$RELEASE_SIGNER_BIN" release-sign' \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "fresh staged mayhem is no longer the release signer"
signer_block="$(
  sed -n '/"\$RELEASE_SIGNER_BIN" release-sign/,/--force/p' \
    "$ROOT_DIR/scripts/package-release.sh"
)"
if grep -F -- '--write-key' <<<"$signer_block" >/dev/null; then
  fail "candidate release signer can replace the pre-provisioned trust anchor"
fi
grep -F 'TRUSTED_RELEASE_KEY="$TMP_DIR/trusted-release-key.json"' \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "package signer does not preserve a pre-signer canonical key snapshot"
grep -F "independent release manifest Ed25519 signature verification failed" \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "package output lacks independent staged-signature verification"
grep -F 'PUBLICATION_DIR="$TMP_DIR/publication"' \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "release outputs are exposed before signature verification completes"
grep -F "CompressionLevel.NoCompression" \
  "$ROOT_DIR/scripts/package-release.sh" >/dev/null ||
  fail "Windows release ZIP no longer uses the deterministic .NET backend"

printf 'release-package-capabilities.test: ok\n'
