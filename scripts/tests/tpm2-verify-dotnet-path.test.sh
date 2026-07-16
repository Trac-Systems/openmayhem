#!/bin/sh
set -eu

root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

mkdir -p "$tmp/home/.dotnet" "$tmp/helper"
cat >"$tmp/home/.dotnet/dotnet" <<'SH'
#!/bin/sh
set -eu
case "${1:-}" in
  --version)
    printf '%s\n' '8.0.100'
    ;;
  restore)
    ;;
  build)
    project="$2"
    output="$(dirname -- "$project")/bin/Release/net8.0/MayhemTpm2Verify"
    mkdir -p "$(dirname -- "$output")"
    cat >"$output" <<'HELPER'
#!/bin/sh
printf '%s\n' '{"ok":true,"source":"fake-user-local-dotnet"}'
HELPER
    chmod +x "$output"
    ;;
  *)
    printf '%s\n' "unexpected fake dotnet invocation: $*" >&2
    exit 1
    ;;
esac
SH
chmod +x "$tmp/home/.dotnet/dotnet"

output="$(
  HOME="$tmp/home" \
  MAYHEM_TPM2_VERIFIER_HELPER_DIR="$tmp/helper" \
  PATH="/usr/bin:/bin:/sbin" \
  "$root/scripts/hardware/mayhem-tpm2-verify-dotnet.sh" <<'JSON'
{}
JSON
)"

[ "$output" = '{"ok":true,"source":"fake-user-local-dotnet"}' ] || {
  printf '%s\n' "unexpected verifier output: $output" >&2
  exit 1
}

printf '%s\n' 'tpm2 verifier user-local dotnet lookup: ok'
