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
shell_source_binary="$(
  sed -n '/^install_source_binary() {/,/^}/p' "$ROOT_DIR/install.sh"
)"
grep -F 'install -m 0755 "$src" "$tmp"' <<<"$shell_source_binary" >/dev/null ||
  fail "Unix source install does not create a fresh executable file"
grep -F 'xattr -d com.apple.quarantine "$tmp"' <<<"$shell_source_binary" >/dev/null ||
  fail "macOS source install does not remove inherited quarantine metadata"
grep -F 'xattr -p com.apple.quarantine "$tmp"' <<<"$shell_source_binary" >/dev/null ||
  fail "macOS source install does not reject retained quarantine metadata"
if grep -F 'spctl' "$ROOT_DIR/install.sh" >/dev/null; then
  fail "Unix source install incorrectly requires Gatekeeper distribution approval"
fi

installer_acceleration_functions="$(
  awk '
    /^llama_cpp_feature_name\(\) \{/ { capture = 1 }
    /^install_source_binary\(\) \{/ { capture = 0 }
    capture { print }
  ' "$ROOT_DIR/install.sh"
)"
for function_name in \
  llama_cpp_feature_name \
  llama_cpp_cuda_toolkit_usable \
  llama_cpp_cuda_library_dirs \
  llama_cpp_vulkan_toolkit_usable \
  linux_llama_cpp_features; do
  grep -F "${function_name}() {" <<<"$installer_acceleration_functions" >/dev/null ||
    fail "Unix source installer is missing $function_name"
done
grep -F '"$resolved" --version' <<<"$installer_acceleration_functions" >/dev/null ||
  fail "Unix source installer accepts an nvcc path without executing it"
for library in \
  libcudart_static.a \
  libcublas_static.a \
  libcublasLt_static.a \
  libculibos.a; do
  grep -F "$library" <<<"$installer_acceleration_functions" >/dev/null ||
    fail "Unix source installer does not require CUDA static library: $library"
done
grep -F 'pkg-config --exists vulkan' <<<"$installer_acceleration_functions" >/dev/null ||
  fail "Unix source installer does not validate Vulkan development metadata"

cuda_fixture="$(mktemp -d)"
mkdir -p "$cuda_fixture/bin" "$cuda_fixture/lib"
cat >"$cuda_fixture/bin/nvcc" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod 0755 "$cuda_fixture/bin/nvcc"
for library in \
  libcudart_static.a \
  libcublas_static.a \
  libcublasLt_static.a \
  libculibos.a; do
  : >"$cuda_fixture/lib/$library"
done
(
  eval "$installer_acceleration_functions"
  uname() {
    case "${1:-}" in
      -s) printf 'Linux\n' ;;
      -m) printf 'x86_64\n' ;;
      *) return 1 ;;
    esac
  }
  export CUDACXX="$cuda_fixture/bin/nvcc"
  export CUDA_LIBRARY_PATH="$cuda_fixture/lib"
  [[ "$(llama_cpp_cuda_library_dirs)" == "$cuda_fixture/lib" ]]
  llama_cpp_cuda_toolkit_usable
) || fail "Unix source installer rejected a complete Debian-style CUDA toolkit"
rm -f "$cuda_fixture/lib/libculibos.a"
if (
  eval "$installer_acceleration_functions"
  uname() {
    case "${1:-}" in
      -s) printf 'Linux\n' ;;
      -m) printf 'x86_64\n' ;;
      *) return 1 ;;
    esac
  }
  export CUDACXX="$cuda_fixture/bin/nvcc"
  export CUDA_LIBRARY_PATH="$cuda_fixture/lib"
  llama_cpp_cuda_toolkit_usable
) >/dev/null 2>&1; then
  rm -rf "$cuda_fixture"
  fail "Unix source installer accepted CUDA without libculibos.a"
fi
rm -rf "$cuda_fixture"
installer_source_backend_functions="$(
  awk '
    /^llama_cpp_source_backend\(\) \{/ { capture = 1 }
    /^install_from_source\(\) \{/ { capture = 0 }
    capture { print }
  ' "$ROOT_DIR/install.sh"
)"
for function_name in \
  llama_cpp_source_backend \
  verify_source_llama_cpp_backend; do
  grep -F "${function_name}() {" <<<"$installer_source_backend_functions" >/dev/null ||
    fail "Unix source installer is missing $function_name"
done

assert_installer_acceleration() {
  local expected="$1"
  local target_arch="$2"
  local host_arch="$3"
  local cuda_status="$4"
  local vulkan_status="$5"
  local configured="${6:-}"
  local actual

  actual="$(
    (
      eval "$installer_acceleration_functions"
      die() {
        printf 'error: %s\n' "$*" >&2
        exit 1
      }
      llama_cpp_cuda_toolkit_usable() {
        return "$cuda_status"
      }
      llama_cpp_vulkan_toolkit_usable() {
        return "$vulkan_status"
      }
      if [[ -n "$configured" ]]; then
        export MAYHEM_LLAMA_CPP_FEATURES="$configured"
      else
        unset MAYHEM_LLAMA_CPP_FEATURES
      fi
      linux_llama_cpp_features Linux "$target_arch" "$host_arch"
    )
  )"
  [[ "$actual" == "$expected" ]] ||
    fail "Unix source acceleration for $target_arch selected '$actual', expected '$expected'"
}

for linux_arch in x86_64 aarch64; do
  assert_installer_acceleration \
    "mayhem-cli/llama-cpp-cuda" "$linux_arch" "$linux_arch" 0 0
  assert_installer_acceleration \
    "mayhem-cli/llama-cpp-vulkan" "$linux_arch" "$linux_arch" 1 0
  assert_installer_acceleration \
    "" "$linux_arch" "$linux_arch" 1 1
  assert_installer_acceleration \
    "" "$linux_arch" "$linux_arch" 0 0 cpu
  assert_installer_acceleration \
    "mayhem-cli/llama-cpp-cuda" "$linux_arch" "$linux_arch" 0 0 cuda
done
assert_installer_acceleration "" x86_64 aarch64 0 0
assert_installer_acceleration \
  "mayhem-cli/llama-cpp-vulkan" x86_64 aarch64 0 0 vulkan

if (
  eval "$installer_acceleration_functions"
  die() { exit 1; }
  llama_cpp_cuda_toolkit_usable() { return 1; }
  llama_cpp_vulkan_toolkit_usable() { return 0; }
  MAYHEM_LLAMA_CPP_FEATURES=cuda \
    linux_llama_cpp_features Linux x86_64 x86_64
) >/dev/null 2>&1; then
  fail "Unix source installer accepted an explicit CUDA build without working nvcc"
fi
for conflicting in cuda,vulkan cpu,cuda; do
  if (
    eval "$installer_acceleration_functions"
    eval "$installer_source_backend_functions"
    die() { exit 1; }
    llama_cpp_cuda_toolkit_usable() { return 0; }
    llama_cpp_vulkan_toolkit_usable() { return 0; }
    export MAYHEM_LLAMA_CPP_FEATURES="$conflicting"
    features="$(linux_llama_cpp_features Linux x86_64 x86_64)"
    llama_cpp_source_backend "$features" "$MAYHEM_LLAMA_CPP_FEATURES"
  ) >/dev/null 2>&1; then
    fail "Unix source installer accepted conflicting backends: $conflicting"
  fi
done

shell_source_build="$(
  sed -n '/^install_from_source() {/,/^}/p' "$ROOT_DIR/install.sh"
)"
grep -F 'linux_llama_cpp_features Linux "$(uname -m)" "$(uname -m)"' \
  <<<"$shell_source_build" >/dev/null ||
  fail "Unix source build does not select Linux llama.cpp acceleration"
grep -F 'cargo_args+=(--features "$llama_cpp_features")' \
  <<<"$shell_source_build" >/dev/null ||
  fail "Unix source build does not pass selected llama.cpp features to Cargo"
grep -F 'export CUDA_LIBRARY_PATH=' <<<"$shell_source_build" >/dev/null ||
  fail "Unix source build does not expose validated CUDA libraries to Cargo"
grep -F 'mayhem-cli/llama-cpp-metal' <<<"$shell_source_build" >/dev/null ||
  fail "Unix source acceleration change removed the existing macOS Metal build"
grep -F 'SOURCE_LLAMA_CPP_BACKEND="metal"' <<<"$shell_source_build" >/dev/null ||
  fail "macOS source build does not record its deterministic Metal backend"
grep -F 'llama_cpp_source_backend \' \
  <<<"$shell_source_build" >/dev/null ||
  fail "Linux source build does not record its selected local backend"

shell_smoke_test="$(
  sed -n '/^smoke_test() {/,/^}/p' "$ROOT_DIR/install.sh"
)"
grep -F 'verify_source_llama_cpp_backend "$SOURCE_LLAMA_CPP_BACKEND"' \
  <<<"$shell_smoke_test" >/dev/null ||
  fail "Unix source install does not verify the selected backend with mayhem doctor"
grep -F -- '--fixture linux-nvidia --gpu-layers 1' \
  <<<"$installer_source_backend_functions" >/dev/null ||
  fail "Unix source CUDA verification does not prove accelerator support"
grep -F -- '--fixture apple-silicon --gpu-layers 1' \
  <<<"$installer_source_backend_functions" >/dev/null ||
  fail "macOS source verification does not prove Metal support"
grep -F -- '--fixture cpu-only --gpu-layers 0' \
  <<<"$installer_source_backend_functions" >/dev/null ||
  fail "Unix source CPU verification is not pinned to CPU execution"
vulkan_verification="$(
  sed -n '/^[[:space:]]*vulkan)/,/^[[:space:]]*;;/p' \
    <<<"$installer_source_backend_functions"
)"
grep -F -- '--fixture cpu-only --gpu-layers 0' \
  <<<"$vulkan_verification" >/dev/null ||
  fail "Unix Vulkan source verification can falsely depend on hwprobe GPU classification"
grep -F 'Vulkan feature build and deterministic CPU fallback' \
  <<<"$installer_source_backend_functions" >/dev/null ||
  fail "Unix Vulkan source verification overclaims live Vulkan execution"

grep -F '"$INSTALL_DIR/mayhem" --version' "$ROOT_DIR/install.sh" >/dev/null ||
  fail "Unix installer does not execute the installed binary version check"

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
powershell_source_binary="$(
  sed -n '/^function Install-SourceBinary {/,/^}/p' "$ROOT_DIR/install.ps1"
)"
grep -F '[System.IO.FileMode]::CreateNew' <<<"$powershell_source_binary" >/dev/null ||
  fail "PowerShell source install does not create a fresh executable file"
grep -F -- '-Stream "Zone.Identifier"' <<<"$powershell_source_binary" >/dev/null ||
  fail "PowerShell source install does not reject inherited Zone.Identifier metadata"
if grep -F 'Unblock-File' "$ROOT_DIR/install.ps1" >/dev/null; then
  fail "PowerShell source install retains a manual unblock dependency"
fi
grep -F '& $mayhem --version' "$ROOT_DIR/install.ps1" >/dev/null ||
  fail "PowerShell installer does not execute the installed binary version check"
for token in \
  'function Test-CudaToolkitUsable' \
  'function Test-VulkanToolkitUsable' \
  'function Resolve-LlamaCppSourceBuild' \
  'Windows ARM64 source builds support the llama.cpp CPU backend only' \
  'function Confirm-SourceLlamaCppBackend' \
  'Confirm-SourceLlamaCppBackend'; do
  grep -F "$token" "$ROOT_DIR/install.ps1" >/dev/null ||
    fail "PowerShell source backend contract is missing: $token"
done

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
grep -Fq \
  'append_default MAYHEM_PAYGATE_INTERNAL_AUTH_SECRET_FILE "$root/.mayhem-local/live-home/paygate/internal-auth.secret"' \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not share one private paygate authentication secret with the admin peer"
grep -Fq \
  "append_default MAYHEM_STRIPE_WORKER_URL 'http://127.0.0.1:11436'" \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not pin the canonical peer to its loopback Stripe worker"
grep -Fq \
  'append_default MAYHEM_STRIPE_CONNECT_CONSENTS_PATH "$root/.mayhem-local/paygate/stripe-connect-consents.jsonl"' \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not persist Stripe OAuth consent state privately"
grep -Fq "append_default MAYHEM_STRIPE_CONNECT_OAUTH_BRIDGE_URL ''" \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not default the optional Stripe OAuth bridge URL"
grep -Fq "append_default MAYHEM_STRIPE_CONNECT_OAUTH_BRIDGE_SECRET_FILE ''" \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not default the optional Stripe OAuth bridge secret file"
grep -Fq 'ensure_private_random_secret_file \' \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not initialize the private paygate authentication secret"

oauth_require_equal="$(
  sed -n '/^require_equal() {/,/^}/p' \
    "$ROOT_DIR/scripts/install-mainnet-systemd.sh"
)"
oauth_require_prefix="$(
  sed -n '/^require_prefix() {/,/^}/p' \
    "$ROOT_DIR/scripts/install-mainnet-systemd.sh"
)"
oauth_require_https_url="$(
  sed -n '/^require_https_url() {/,/^}/p' \
    "$ROOT_DIR/scripts/install-mainnet-systemd.sh"
)"
oauth_validator="$(
  sed -n '/^validate_stripe_oauth_config() {/,/^}/p' \
    "$ROOT_DIR/scripts/install-mainnet-systemd.sh"
)"
[[ -n "$oauth_require_equal" && -n "$oauth_require_prefix" &&
  -n "$oauth_require_https_url" && -n "$oauth_validator" ]] ||
  fail "mainnet Stripe OAuth installer validation functions are missing"

run_oauth_validation() {
  local expected_secret_calls="$1"
  shift
  (
    local oauth_client_id='' oauth_redirect_url='' oauth_bridge_url=''
    local oauth_bridge_secret_file='' oauth_token_url='https://connect.stripe.com/oauth/token'
    local oauth_secret_calls=0 assignment key value
    root='/opt/mayhem'

    for assignment in "$@"; do
      key="${assignment%%=*}"
      value="${assignment#*=}"
      case "$key" in
        client_id) oauth_client_id="$value" ;;
        redirect_url) oauth_redirect_url="$value" ;;
        bridge_url) oauth_bridge_url="$value" ;;
        bridge_secret_file) oauth_bridge_secret_file="$value" ;;
        token_url) oauth_token_url="$value" ;;
        *) fail "unknown Stripe OAuth test field: $key" ;;
      esac
    done

    env_value() {
      case "$1" in
        MAYHEM_STRIPE_CONNECT_OAUTH_CLIENT_ID) printf '%s' "$oauth_client_id" ;;
        MAYHEM_STRIPE_CONNECT_OAUTH_REDIRECT_URL) printf '%s' "$oauth_redirect_url" ;;
        MAYHEM_STRIPE_CONNECT_OAUTH_BRIDGE_URL) printf '%s' "$oauth_bridge_url" ;;
        MAYHEM_STRIPE_CONNECT_OAUTH_BRIDGE_SECRET_FILE) printf '%s' "$oauth_bridge_secret_file" ;;
        MAYHEM_STRIPE_CONNECT_OAUTH_TOKEN_URL) printf '%s' "$oauth_token_url" ;;
        *) return 1 ;;
      esac
    }
    ensure_private_random_secret_file() {
      [[ "$1" == 'MAYHEM_STRIPE_CONNECT_OAUTH_BRIDGE_SECRET_FILE' ]] || return 1
      [[ "$2" == '/opt/mayhem/.mayhem-local/live-home/paygate/stripe-oauth-bridge.secret' ]] || return 1
      [[ "$oauth_bridge_secret_file" == "$2" ]] || return 1
      oauth_secret_calls=$((oauth_secret_calls + 1))
    }
    require_file_env() {
      [[ "$1" == 'MAYHEM_STRIPE_CONNECT_OAUTH_BRIDGE_SECRET_FILE' &&
        -n "$oauth_bridge_secret_file" ]]
    }

    eval "$oauth_require_equal"
    eval "$oauth_require_prefix"
    eval "$oauth_require_https_url"
    eval "$oauth_validator"
    validate_stripe_oauth_config || return $?
    [[ "$oauth_secret_calls" == "$expected_secret_calls" ]]
  )
}

oauth_complete=(
  'client_id=ca_live_test'
  'redirect_url=https://www.openmayhem.ai/api/stripe/connect/oauth/callback'
  'bridge_url=https://www.openmayhem.ai/api/stripe/connect/oauth'
  'bridge_secret_file=/opt/mayhem/.mayhem-local/live-home/paygate/stripe-oauth-bridge.secret'
)
run_oauth_validation 0 ||
  fail "mainnet live install rejects an entirely unconfigured optional Stripe OAuth bridge"
run_oauth_validation 1 "${oauth_complete[@]}" ||
  fail "mainnet live install rejects a complete canonical Stripe OAuth bridge configuration"
if run_oauth_validation 0 'client_id=ca_partial' >/dev/null 2>&1; then
  fail "mainnet live install accepts a partial Stripe OAuth bridge configuration"
fi
if run_oauth_validation 0 "${oauth_complete[@]}" \
  'redirect_url=https://www.openmayhem.ai/wrong/callback' >/dev/null 2>&1; then
  fail "mainnet live install accepts a callback outside the configured OAuth bridge"
fi
if run_oauth_validation 0 "${oauth_complete[@]}" \
  'bridge_url=http://www.openmayhem.ai/api/stripe/connect/oauth' \
  'redirect_url=http://www.openmayhem.ai/api/stripe/connect/oauth/callback' >/dev/null 2>&1; then
  fail "mainnet live install accepts a non-HTTPS Stripe OAuth bridge"
fi
if run_oauth_validation 0 "${oauth_complete[@]}" \
  'bridge_url=https://operator@www.openmayhem.ai/api/stripe/connect/oauth' \
  'redirect_url=https://operator@www.openmayhem.ai/api/stripe/connect/oauth/callback' >/dev/null 2>&1; then
  fail "mainnet live install accepts credentials in the Stripe OAuth bridge URL"
fi
if run_oauth_validation 0 "${oauth_complete[@]}" \
  'bridge_url=https:///api/stripe/connect/oauth' \
  'redirect_url=https:///api/stripe/connect/oauth/callback' >/dev/null 2>&1; then
  fail "mainnet live install accepts a Stripe OAuth bridge URL without a host"
fi
if run_oauth_validation 0 "${oauth_complete[@]}" \
  'bridge_url=https://paygate.trac.network/api/stripe/connect/oauth' \
  'redirect_url=https://paygate.trac.network/api/stripe/connect/oauth/callback' >/dev/null 2>&1; then
  fail "mainnet live install accepts the retired paygate.trac.network OAuth bridge"
fi
if run_oauth_validation 0 "${oauth_complete[@]}" \
  'token_url=https://example.invalid/oauth/token' >/dev/null 2>&1; then
  fail "mainnet live install accepts a non-official Stripe OAuth token endpoint"
fi
grep -Fq '"$root/.mayhem-local/settlement/tap" \' \
  "$ROOT_DIR/scripts/install-mainnet-systemd.sh" ||
  fail "mainnet install does not explicitly own the TAP settlement spool root"

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
  grep -F ':-0.2.35}' "$harness" >/dev/null ||
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
