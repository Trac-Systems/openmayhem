#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fail() {
  printf 'bootstrap-installer-contract.test: %s\n' "$*" >&2
  exit 1
}

for installer in "$ROOT_DIR/install.sh" "$ROOT_DIR/install.ps1"; do
  grep -F "release manifest Ed25519 signature verification failed" "$installer" >/dev/null ||
    fail "$installer does not authenticate the detached release signature"
  grep -F "trusted-release-keys" "$installer" >/dev/null ||
    fail "$installer does not provision updater release trust"
  grep -F "release-floor.json" "$installer" >/dev/null ||
    fail "$installer does not require a provisioned anti-rollback floor"
  grep -F "apply-staged" "$installer" >/dev/null ||
    fail "$installer does not delegate activation to the staged mayhem CLI"
  grep -F "signed installs cannot use unpinned latest" "$installer" >/dev/null ||
    fail "$installer permits an unbound latest signed install"
  grep -F "does not match requested version" "$installer" >/dev/null ||
    fail "$installer does not bind the exact requested release version"
  grep -F "below protected anti-rollback floor" "$installer" >/dev/null ||
    fail "$installer does not compare the authenticated version with the existing floor"
  grep -F "snapshotted signed release inputs into private installer state" "$installer" >/dev/null ||
    fail "$installer does not use private signed-input snapshots"
  grep -F "2.0.4" "$installer" >/dev/null ||
    fail "$installer does not pin the canonical Pear version"
done

grep -F "fs.constants.O_NOFOLLOW" "$ROOT_DIR/install.sh" >/dev/null ||
  fail "Unix signed-input snapshots can follow symbolic links"
grep -F "FILE_FLAG_OPEN_REPARSE_POINT" "$ROOT_DIR/install.ps1" >/dev/null ||
  fail "Windows signed-input snapshots can follow reparse points"
grep -F "SetAccessRuleProtection(\$true, \$false)" "$ROOT_DIR/install.ps1" >/dev/null ||
  fail "Windows installer temp directories do not use a private ACL"
for token in \
  'HashSet[string]' \
  'StringComparer]::OrdinalIgnoreCase' \
  'ExternalAttributes' \
  'duplicate or case-colliding entry' \
  'traversal or a non-portable entry path' \
  'link, special file, or ambiguous file type'; do
  grep -F "$token" "$ROOT_DIR/install.ps1" >/dev/null ||
    fail "Windows signed ZIP validation is missing: $token"
done

if grep -Eini \
  'Start-Process.{0,40}-Verb[[:space:]]+RunAs|runas(\.exe)?|net[[:space:]]+localgroup|Set-ExecutionPolicy[[:space:]]+-Scope[[:space:]]+(LocalMachine|MachinePolicy)|\[Environment\]::SetEnvironmentVariable\([^,]+,[^,]+,[[:space:]]*"Machine"\)' \
  "$ROOT_DIR/install.ps1" >/dev/null; then
  fail "Windows bootstrap contains an administrator or machine-wide install path"
fi

shell_main="$(
  sed -n '/^if \[\[ "\$FROM_SOURCE" == "1" \]\]; then/,/^fi$/p' \
    "$ROOT_DIR/install.sh"
)"
grep -F "hydrate_runtime_assets" <<<"$shell_main" >/dev/null ||
  fail "source install no longer hydrates its development runtime"
shell_intercom_hydration="$(
  sed -n '/^hydrate_intercom_package() {/,/^}/p' "$ROOT_DIR/install.sh"
)"
grep -F 'npm ci --omit=dev --install-links=true' \
  <<<"$shell_intercom_hydration" >/dev/null ||
  fail "Unix source install does not use root-authoritative Intercom hydration"
grep -F 'verify-intercom-dependency-topology.mjs' \
  <<<"$shell_intercom_hydration" >/dev/null ||
  fail "Unix source install does not verify the Intercom dependency topology"
grep -F 'materialize-local-dependencies.mjs' \
  <<<"$shell_intercom_hydration" >/dev/null ||
  fail "Unix source install does not restore exact pinned runtime files"
shell_runtime_hydration="$(
  sed -n '/^hydrate_runtime_assets() {/,/^}/p' "$ROOT_DIR/install.sh"
)"
if grep -E 'intercom/trac/(msb|trac-peer)' <<<"$shell_runtime_hydration" >/dev/null; then
  fail "Unix source install retains nested Intercom hydration"
fi
artifact_branch="${shell_main#*else}"
if grep -F "hydrate_runtime_assets" <<<"$artifact_branch" >/dev/null; then
  fail "artifact install still mutates authenticated assets with npm"
fi
grep -F $'install_from_artifact\n  ensure_pear' <<<"$artifact_branch" >/dev/null ||
  fail "Unix artifact activation does not finish before Pear installation"
grep -F 'npm install -g "pear@$PEAR_VERSION"' "$ROOT_DIR/install.sh" >/dev/null ||
  fail "Unix Pear bootstrap is not pinned"
if grep -E 'npm install -g ("?pear"?)([[:space:]]|$)' "$ROOT_DIR/install.sh" >/dev/null; then
  fail "Unix Pear bootstrap retains an unversioned npm install"
fi

powershell_main="$(
  sed -n '/^[[:space:]]*if (\$FromSource) {/,/^[[:space:]]*Install-Opencode$/p' \
    "$ROOT_DIR/install.ps1"
)"
grep -F "Hydrate-RuntimeAssets" <<<"$powershell_main" >/dev/null ||
  fail "PowerShell source install no longer hydrates its development runtime"
powershell_intercom_hydration="$(
  sed -n '/^function Invoke-IntercomNpmInstall {/,/^}/p' "$ROOT_DIR/install.ps1"
)"
grep -F '"--install-links=true"' <<<"$powershell_intercom_hydration" >/dev/null ||
  fail "PowerShell source install does not force physical Intercom dependencies"
grep -F 'verify-intercom-dependency-topology.mjs' \
  <<<"$powershell_intercom_hydration" >/dev/null ||
  fail "PowerShell source install does not verify the Intercom dependency topology"
grep -F 'materialize-local-dependencies.mjs' \
  <<<"$powershell_intercom_hydration" >/dev/null ||
  fail "PowerShell source install does not restore exact pinned runtime files"
powershell_runtime_hydration="$(
  sed -n '/^function Hydrate-RuntimeAssets {/,/^}/p' "$ROOT_DIR/install.ps1"
)"
if grep -E 'intercom.*trac.*(msb|trac-peer)' \
  <<<"$powershell_runtime_hydration" >/dev/null; then
  fail "PowerShell source install retains nested Intercom hydration"
fi
powershell_artifact="${powershell_main#*else}"
if grep -F "Hydrate-RuntimeAssets" <<<"$powershell_artifact" >/dev/null; then
  fail "PowerShell artifact install still mutates authenticated assets with npm"
fi
grep -F $'Install-FromArtifact\n        Ensure-Pear' <<<"$powershell_artifact" >/dev/null ||
  fail "PowerShell artifact activation does not finish before Pear installation"
grep -F '& npm install -g "pear@$PearVersion"' "$ROOT_DIR/install.ps1" >/dev/null ||
  fail "PowerShell Pear bootstrap is not pinned"
if grep -E '& npm install -g "?pear"?([[:space:]]|$)' "$ROOT_DIR/install.ps1" >/dev/null; then
  fail "PowerShell Pear bootstrap retains an unversioned npm install"
fi

mainnet_intercom_hydration="$(
  sed -n '/^hydrate_intercom_package() {/,/^}/p' \
    "$ROOT_DIR/scripts/install-mainnet-systemd.sh"
)"
grep -F -- '--install-links=true' <<<"$mainnet_intercom_hydration" >/dev/null ||
  fail "mainnet source install does not force physical Intercom dependencies"
grep -F 'verify-intercom-dependency-topology.mjs' \
  <<<"$mainnet_intercom_hydration" >/dev/null ||
  fail "mainnet source install does not verify the Intercom dependency topology"
grep -F 'materialize-local-dependencies.mjs' \
  <<<"$mainnet_intercom_hydration" >/dev/null ||
  fail "mainnet source install does not restore exact pinned runtime files"
grep -F '"$dir/trac/msb/node_modules" "$dir/trac/trac-peer/node_modules"' \
  <<<"$mainnet_intercom_hydration" >/dev/null ||
  fail "mainnet source install does not remove stale nested hydration"
if grep -E '^hydrate_npm_package "\$repo/intercom(/trac/[^"]*)?"' \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" >/dev/null; then
  fail "mainnet source install retains separate Intercom npm hydration"
fi

shell_signed="$(
  sed -n '/^install_signed_release() {/,/^}/p' "$ROOT_DIR/install.sh"
)"
shell_identity_line="$(grep -n 'identity="$(verify_bootstrap_release_identity' <<<"$shell_signed" | cut -d: -f1)"
shell_extract_line="$(grep -n '^  extract_authenticated_bootstrap' <<<"$shell_signed" | cut -d: -f1)"
[[ -n "$shell_identity_line" && -n "$shell_extract_line" &&
  "$shell_identity_line" -lt "$shell_extract_line" ]] ||
  fail "Unix floor/identity preflight does not precede archive extraction"

powershell_signed="$(
  sed -n '/^function Install-SignedRelease {/,/^}/p' "$ROOT_DIR/install.ps1"
)"
powershell_identity_line="$(
  grep -n 'Get-BootstrapReleaseIdentity' <<<"$powershell_signed" | cut -d: -f1
)"
powershell_extract_line="$(
  grep -n 'Expand-AuthenticatedBootstrap' <<<"$powershell_signed" | cut -d: -f1
)"
[[ -n "$powershell_identity_line" && -n "$powershell_extract_line" &&
  "$powershell_identity_line" -lt "$powershell_extract_line" ]] ||
  fail "Windows floor/identity preflight does not precede archive extraction"

grep -F "MAYHEM_ALLOW_UNVERIFIED has been removed" "$ROOT_DIR/install.sh" >/dev/null ||
  fail "Unix bootstrap retains the unverified production bypass"
grep -F "MAYHEM_ALLOW_UNVERIFIED have been removed" "$ROOT_DIR/install.ps1" >/dev/null ||
  fail "Windows bootstrap retains the unverified production bypass"

for harness in \
  "$ROOT_DIR/scripts/macos-opencode-role-check.sh" \
  "$ROOT_DIR/scripts/docker-opencode-role-check.sh"; do
  grep -F ':-0.2.23}' "$harness" >/dev/null ||
    fail "$harness does not default to canonical release semver"
  grep -F -- '--unsigned-layout' "$harness" >/dev/null ||
    fail "$harness does not explicitly request the unsigned test layout"
done
if MAYHEM_MACOS_OPENCODE_VERSION=not-semver \
  "$ROOT_DIR/scripts/macos-opencode-role-check.sh" >/dev/null 2>&1; then
  fail "macOS opencode harness accepted a noncanonical package version"
fi
if MAYHEM_DOCKER_VERSION=not-semver \
  "$ROOT_DIR/scripts/docker-opencode-role-check.sh" >/dev/null 2>&1; then
  fail "Docker opencode harness accepted a noncanonical package version"
fi

printf 'bootstrap-installer-contract.test: ok\n'
