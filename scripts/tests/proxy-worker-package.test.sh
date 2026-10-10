#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export MAYHEM_PACKAGE_RELEASE_SOURCE_ONLY=1
# Source functions only: no builds, signing, installs or release publication.
source "$ROOT_DIR/scripts/package-release.sh"
fail() { printf 'proxy-worker-package.test: %s\n' "$*" >&2; exit 1; }
expect_failure() { if ("$@") >/dev/null 2>&1; then fail "unexpected success: $1"; fi; }
tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-proxy-package.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

printf '%s\n' "${BINS[@]}" > "$tmp/expected"
[[ "$(grep -c '^mayhem-proxy-worker$' "$tmp/expected")" == 1 ]] || fail 'missing or repeated worker'
for file in install.sh scripts/macos-install-check.sh scripts/docker-linux-install-check.sh; do
  sed -n '/^BINS=(/,/^)/p' "$ROOT_DIR/$file" | sed '1d;$d;s/^  //' > "$tmp/actual"
  cmp "$tmp/expected" "$tmp/actual" || fail "binary inventory drift: $file"
done
node - "$ROOT_DIR" "$tmp/expected" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');
const root = process.argv[2];
const expected = fs.readFileSync(process.argv[3], 'utf8').trim().split('\n');
const updater = fs.readFileSync(path.join(root, 'crates/mayhem-cli/src/release_bundle.rs'), 'utf8');
const requiredBlock = updater.match(/const REQUIRED_RELEASE_BINARY_BASE_NAMES: &\[&str\] = &\[([\s\S]*?)\];/)?.[1];
const required = [...(requiredBlock ?? '').matchAll(/"([a-z-]+)"/g)].map(x => x[1]);
required.push(updater.match(/const PROXY_WORKER_BINARY_BASE_NAME: &str = "([a-z-]+)";/)?.[1]);
if (JSON.stringify(required.sort()) !== JSON.stringify([...expected].sort())) throw new Error('signed updater required binary inventory drift');
const ps = fs.readFileSync(path.join(root, 'install.ps1'), 'utf8');
const block = ps.match(/^\$Bins = @\(([\s\S]*?)^\)/m)?.[1];
const actual = [...(block ?? '').matchAll(/"([a-z-]+)"/g)].map(x => x[1]);
if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error('PowerShell installer inventory drift');
const ci = fs.readFileSync(path.join(root, '.github/workflows/source-build-evidence.yml'), 'utf8');
const ciBins = ci.match(/bins=\(([^)]+)\)/)?.[1].trim().split(/\s+/);
if (JSON.stringify(ciBins) !== JSON.stringify(expected)) throw new Error('native-build evidence inventory drift');
NODE

# The current source package has the authenticated marker used by the updater.
# Exercise its real tracked allowlist without dependency hydration/downloads.
copy_tracked_allowlist "$tmp/runtime" "${INTERCOM_SOURCE_ALLOWLIST[@]}"
[[ -f "$tmp/runtime/intercom/contract/proxy-protocol.js" ]] || fail 'proxy runtime marker not staged'
cmp "$ROOT_DIR/intercom/contract/proxy-protocol.js" "$tmp/runtime/intercom/contract/proxy-protocol.js"
# TAP returns must load from the installed Intercom layout, without depending
# on a developer's sibling contracts checkout or its node_modules symlink.
[[ -f "$tmp/runtime/intercom/scripts/proxy-admission-refund-tap-transaction.mjs" ]] || fail 'TAP return codec omitted'
ln -s "$ROOT_DIR/intercom/node_modules" "$tmp/runtime/intercom/node_modules"
node --input-type=module - "$tmp/runtime/intercom" <<'NODE'
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
const root=process.argv[2];
const pkg=JSON.parse(fs.readFileSync(path.join(root,'package.json'),'utf8'));
if(pkg.dependencies.ethers!=='6.17.0')throw Error('TAP signing dependency is not pinned');
await import(pathToFileURL(path.join(root,'scripts/proxy-admission-refund-tap-runtime.mjs')));
const {main}=await import(pathToFileURL(path.join(root,'scripts/proxy-admission-refund-worker.mjs')));
let disabled=false;try{await main({});}catch(e){disabled=/disabled/.test(e.message);}
if(!disabled)throw Error('packaged return worker enabled implicitly');
NODE

VERSION=0.2.999
BUILT_AT=2026-10-10T00:00:00Z
SOURCE_GIT_SHA=0123456789abcdef0123456789abcdef01234567
printf '{"release_version":"%s","assets":[]}\n' "$VERSION" > "$tmp/intercom.json"
for TARGET in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-msvc aarch64-pc-windows-msvc; do
  BIN_EXT=""
  [[ "$TARGET" != *-windows-* ]] || BIN_EXT=.exe
  built="$tmp/$TARGET-built"
  stage="$tmp/$TARGET-stage"
  mkdir -p "$built"
  for bin in "${BINS[@]}"; do printf 'fixture %s %s\n' "$TARGET" "$bin" > "$built/$bin$BIN_EXT"; done
  stage_release_binaries "$built" "$stage"
  cmp "$built/mayhem-proxy-worker$BIN_EXT" "$stage/bin/mayhem-proxy-worker$BIN_EXT"
  [[ -x "$stage/bin/mayhem-proxy-worker$BIN_EXT" ]] || fail 'worker is not executable'
  mkdir -p "$stage/share/mayhem/intercom/contract"
  cp "$tmp/runtime/intercom/contract/proxy-protocol.js" "$stage/share/mayhem/intercom/contract/proxy-protocol.js"
  write_release_manifest "$stage" "$tmp/intercom.json" "$tmp/$TARGET.json"
  node - "$tmp/$TARGET.json" "$stage" "$BIN_EXT" <<'NODE'
const fs = require('node:fs');
const crypto = require('node:crypto');
const manifest = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const name = `mayhem-proxy-worker${process.argv[4]}`;
const binary = manifest.binaries.find(b => b.name === name);
const hash = crypto.createHash('sha256').update(fs.readFileSync(`${process.argv[3]}/bin/${name}`)).digest('hex');
const marker = 'share/mayhem/intercom/contract/proxy-protocol.js';
const markerHash = crypto.createHash('sha256').update(fs.readFileSync(`${process.argv[3]}/${marker}`)).digest('hex');
if (!manifest.assets.some(a => a.path === marker && a.sha256 === markerHash)) throw new Error('proxy runtime marker missing signed inventory binding');
if (!binary || binary.path !== `bin/${name}` || binary.sha256 !== hash ||
    !manifest.assets.some(a => a.path === binary.path && a.sha256 === hash)) {
  throw new Error('worker missing exact outer-manifest hash binding');
}
NODE
  rm "$built/mayhem-proxy-worker$BIN_EXT"
  expect_failure stage_release_binaries "$built" "$tmp/$TARGET-missing"
  rm "$stage/bin/mayhem-proxy-worker$BIN_EXT"
  expect_failure write_release_manifest "$stage" "$tmp/intercom.json" "$tmp/$TARGET-missing.json"
done

# Exercise the actual Unix unsigned-artifact copy path without installer startup.
eval "$(sed -n '/^verified_package_file() {/,/^}/p' "$ROOT_DIR/install.sh")"
eval "$(sed -n '/^copy_artifact_bins() {/,/^}/p' "$ROOT_DIR/install.sh")"
mkdir -p "$tmp/install-source/bin"
VERIFIED_PACKAGE_FILES=""
for bin in "${BINS[@]}"; do
  printf '%s\n' "$bin" > "$tmp/install-source/bin/$bin"
  VERIFIED_PACKAGE_FILES+="bin/$bin"$'\n'
done
INSTALL_DIR="$tmp/installed"
copy_artifact_bins "$tmp/install-source"
cmp "$tmp/install-source/bin/mayhem-proxy-worker" "$INSTALL_DIR/mayhem-proxy-worker"
[[ -x "$INSTALL_DIR/mayhem-proxy-worker" ]] || fail 'installed worker is not executable'
VERIFIED_PACKAGE_FILES="$(printf '%s' "$VERIFIED_PACKAGE_FILES" | grep -v 'proxy-worker')"
INSTALL_DIR="$tmp/unverified"
expect_failure copy_artifact_bins "$tmp/install-source"
[[ ! -e "$INSTALL_DIR/mayhem-proxy-worker" ]] || fail 'copied worker without verified inventory entry'
printf 'proxy-worker-package.test: staging, hash binding, omission refusal and Unix copy passed\n'
