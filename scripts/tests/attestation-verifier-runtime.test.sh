#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="${MAYHEM_ATTESTATION_VERIFIER_BIN:-$ROOT_DIR/target/debug/mayhem-attestation-verifier}"
SKIP_SOURCE_CHECKS="${MAYHEM_ATTESTATION_VERIFIER_SKIP_SOURCE_CHECKS:-0}"

fail() {
  printf 'attestation-verifier-runtime.test: %s\n' "$*" >&2
  exit 1
}

case "$SKIP_SOURCE_CHECKS" in
  0 | 1) ;;
  *) fail "MAYHEM_ATTESTATION_VERIFIER_SKIP_SOURCE_CHECKS must be 0 or 1" ;;
esac

if [[ ! -x "$BIN" ]]; then
  [[ "$SKIP_SOURCE_CHECKS" == "0" ]] ||
    fail "clean-install verifier binary is missing or not executable: $BIN"
  (cd "$ROOT_DIR" && cargo build -p mayhem-attestation-verifier)
fi

identity_output="$("$BIN" --identity </dev/null)"
[[ "${#identity_output}" -le 4096 ]] || fail "identity output exceeds its 4 KiB bound"
grep -q '"verifier_id":"mayhem-attestation-verifier"' <<<"$identity_output" ||
  fail "identity output has the wrong verifier id"
grep -q '"version":1' <<<"$identity_output" ||
  fail "identity output has the wrong verifier version"
for profile in \
  amd_sev_snp_vcek_v1 \
  intel_tdx_dcap_v1 \
  nvidia_nras_composite_v1 \
  nvidia_nvtrust_offline_composite_v1; do
  grep -q "\"$profile\":\\[1\\]" <<<"$identity_output" ||
    fail "identity output omits $profile schema 1"
done
grep -q '"public_trust_source":"authenticated_admin_policy_input"' <<<"$identity_output" ||
  fail "identity output does not pin authenticated policy trust"
if grep -Eq 'executable_sha256|"platform"|endpoint|jwks|trust_root' <<<"$identity_output"; then
  fail "identity output contains platform-specific or caller-supplied trust authority"
fi

empty_output="$("$BIN" </dev/null)"
grep -q '"ok":false' <<<"$empty_output" || fail "empty stdin did not produce a fail-closed verdict"
grep -q 'strict verifier input JSON is invalid' <<<"$empty_output" ||
  fail "empty stdin did not report strict JSON rejection"

argument_output="$("$BIN" --trust-root /tmp/provider-root </dev/null)"
grep -q '"ok":false' <<<"$argument_output" || fail "manual argument did not fail closed"
grep -q 'only accepts --identity' <<<"$argument_output" ||
  fail "manual trust argument was not explicitly rejected"

oversize_file="$(mktemp "${TMPDIR:-/tmp}/mayhem-av2-oversize.XXXXXX")"
dd if=/dev/zero of="$oversize_file" bs=1048576 count=9 2>/dev/null
oversize_output="$("$BIN" <"$oversize_file")"
rm -f "$oversize_file"
grep -q 'input exceeds the 8 MiB limit' <<<"$oversize_output" ||
  fail "oversized stdin was not rejected by the executable boundary"

install_dir="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-av2-install.XXXXXX")"
cleanup_install() {
  chmod u+w "$install_dir" 2>/dev/null || true
  rm -rf "$install_dir"
}
trap cleanup_install EXIT
cp "$BIN" "$install_dir/mayhem-attestation-verifier"
chmod 0555 "$install_dir/mayhem-attestation-verifier"
chmod 0555 "$install_dir"

run_clean=(
  env -i
  HOME="${TMPDIR:-/tmp}"
  TMPDIR="${TMPDIR:-/tmp}"
  PATH=/usr/bin:/bin
  "$install_dir/mayhem-attestation-verifier"
)
if [[ "$(id -u)" -eq 0 ]] && command -v setpriv >/dev/null 2>&1; then
  clean_output="$(setpriv --reuid=65534 --regid=65534 --clear-groups "${run_clean[@]}" </dev/null)"
else
  clean_output="$("${run_clean[@]}" </dev/null)"
fi
grep -q '"ok":false' <<<"$clean_output" ||
  fail "clean standard-user execution did not produce a verdict"

if [[ "$SKIP_SOURCE_CHECKS" == "0" ]]; then
  if grep -Eq 'std::process::Command|Command::new|std::fs|fs::|File::|OpenOptions|PathBuf|powershell|python|dotnet|sudo|runas' \
    "$ROOT_DIR"/crates/mayhem-attestation-verifier/src/*.rs; then
    fail "verifier runtime contains an external-command, filesystem-trust, or elevation dependency"
  fi

  bins_block="$(sed -n '/^BINS=(/,/^)/p' "$ROOT_DIR/scripts/package-release.sh")"
  grep -qx '  mayhem-attestation-verifier' <<<"$bins_block" ||
    fail "release package BINS omits mayhem-attestation-verifier"
  grep -q '"mayhem-attestation-verifier"' "$ROOT_DIR/crates/mayhem-cli/src/release_bundle.rs" ||
    fail "release activation does not require the verifier sibling binary"
fi

printf 'attestation-verifier-runtime.test: ok\n'
