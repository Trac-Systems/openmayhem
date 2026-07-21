#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${MAYHEM_MACOS_OPENCODE_VERSION:-0.2.30}"
DIST_REL="${MAYHEM_MACOS_OPENCODE_DIST_REL:-dist/macos-opencode-role-check}"
DIST_DIR="${MAYHEM_MACOS_OPENCODE_DIST_DIR:-$ROOT_DIR/$DIST_REL}"
SKIP_BUILD="${MAYHEM_MACOS_OPENCODE_SKIP_BUILD:-0}"
WORK_ROOT=""
GATEWAY_PID=""

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/macos-opencode-role-check.sh

Build a macOS release archive, install it into a temporary clean HOME, start the
installed gateway, and run:
  - mayhem test --sync-models as role=provider
  - mayhem test --sync-models as role=user
  - mayhem test --sync-models --no-opencode-run against a fresh config

This is local macOS evidence for P8.2. It does not replace the formal
provider/user reference-machine acceptance gate.

Environment:
  MAYHEM_MACOS_OPENCODE_VERSION       Artifact version string
                                      (default: 0.2.30; canonical semver is required)
  MAYHEM_MACOS_OPENCODE_DIST_DIR      Dist output directory
                                      (default: dist/macos-opencode-role-check)
  MAYHEM_MACOS_OPENCODE_SKIP_BUILD    Use existing target/release binaries when set to 1
  MAYHEM_MACOS_OPENCODE_KEEP_TMP      Keep temporary install root when set to 1
  MAYHEM_MACOS_GATEWAY_PORT           Gateway port to use; random when unset
USAGE
}

cleanup() {
  if [[ -n "$GATEWAY_PID" ]]; then
    kill "$GATEWAY_PID" >/dev/null 2>&1 || true
    wait "$GATEWAY_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$WORK_ROOT" && "${MAYHEM_MACOS_OPENCODE_KEEP_TMP:-0}" != "1" ]]; then
    rm -rf "$WORK_ROOT"
  fi
}
trap cleanup EXIT

free_port() {
  node -e 'const net=require("net"); const s=net.createServer(); s.listen(0,"127.0.0.1",()=>{console.log(s.address().port); s.close();});'
}

validate_role_report() {
  node - "$1" "$2" "$3" <<'NODE'
const fs = require('fs');
const [reportPath, role, configPath] = process.argv.slice(2);
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
const assert = (condition, message) => {
  if (!condition) throw new Error(`${reportPath}: ${message}`);
};

assert(report.ok === true, 'test report did not pass');
assert(report.role === role, `expected role ${role}, got ${report.role}`);
assert(report.peer?.skipped === true, 'peer health was not skipped');
assert(report.opencode?.run?.skipped !== true, 'opencode run was skipped');
assert(report.opencode?.run?.tool_use_seen === true, 'opencode tool use marker missing');
assert(report.opencode?.run?.marker_seen === true, 'opencode text marker missing');
assert(typeof report.expected_epoch_evidence_key === 'string', 'missing epoch evidence key');
assert(report.expected_epoch_evidence_key.startsWith('ev/use/'), 'unexpected epoch evidence key');
assert(config.provider?.other, 'existing non-Mayhem provider was removed');
assert(config.provider?.mayhem, 'Mayhem provider was not merged');
assert(config.model === 'other/other-model', 'existing default model was clobbered');
assert(config.theme === 'system', 'existing theme was clobbered');
NODE
}

validate_fresh_report() {
  node - "$1" "$2" <<'NODE'
const fs = require('fs');
const [reportPath, configPath] = process.argv.slice(2);
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
const assert = (condition, message) => {
  if (!condition) throw new Error(`${reportPath}: ${message}`);
};

assert(report.ok === true, 'test report did not pass');
assert(report.peer?.skipped === true, 'peer health was not skipped');
assert(report.opencode?.run?.skipped === true, 'opencode run should be skipped');
assert(report.opencode?.config?.default_model_added === true, 'fresh default Mayhem model was not added');
assert(config.provider?.mayhem, 'Mayhem provider was not written');
assert(typeof config.model === 'string' && config.model.startsWith('mayhem/'), 'fresh default model is not Mayhem');
NODE
}

run_role() {
  role="$1"
  gateway_url="$2"
  install_dir="$3"
  home="$WORK_ROOT/mayhem-$role-home"
  config_home="$WORK_ROOT/mayhem-$role-config"
  report="$WORK_ROOT/mayhem-$role-test.json"
  opencode_config="$config_home/opencode/opencode.json"

  mkdir -p "$home" "$config_home/opencode"
  cat >"$home/config.toml" <<EOF
[role]
mode = "$role"

[network]
gateway_url = "$gateway_url"
rpc_url = "http://127.0.0.1:49223/v1"
EOF
  cat >"$opencode_config" <<'EOF'
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "other": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Other",
      "models": {
        "other-model": {
          "name": "other-model"
        }
      }
    }
  },
  "model": "other/other-model",
  "enabled_providers": ["other"],
  "theme": "system"
}
EOF

  "$install_dir/mayhem" test \
    --home "$home" \
    --gateway-url "$gateway_url" \
    --skip-peer-health \
    --sync-models \
    --opencode-config "$opencode_config" \
    --opencode-bin "$install_dir/opencode" \
    --timeout-seconds 90 \
    --json \
    >"$report"

  cat "$report"
  validate_role_report "$report" "$role" "$opencode_config"
}

run_fresh_config_check() {
  gateway_url="$1"
  install_dir="$2"
  home="$WORK_ROOT/mayhem-fresh-home"
  config_home="$WORK_ROOT/mayhem-fresh-config"
  report="$WORK_ROOT/mayhem-fresh-test.json"
  opencode_config="$config_home/opencode/opencode.json"

  mkdir -p "$home" "$config_home/opencode"
  cat >"$home/config.toml" <<EOF
[role]
mode = "user"

[network]
gateway_url = "$gateway_url"
rpc_url = "http://127.0.0.1:49223/v1"
EOF

  "$install_dir/mayhem" test \
    --home "$home" \
    --gateway-url "$gateway_url" \
    --skip-peer-health \
    --sync-models \
    --opencode-config "$opencode_config" \
    --opencode-bin "$install_dir/opencode" \
    --no-opencode-run \
    --timeout-seconds 90 \
    --json \
    >"$report"

  cat "$report"
  validate_fresh_report "$report" "$opencode_config"
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

[[ "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
  die "MAYHEM_MACOS_OPENCODE_VERSION must be canonical semver"
[[ "$(uname -s)" == "Darwin" ]] || die "macOS opencode role check must run on Darwin"
command -v node >/dev/null 2>&1 || die "node is required for JSON validation"
command -v curl >/dev/null 2>&1 || die "curl is required"

package_args=(--version "$VERSION" --out-dir "$DIST_DIR")
package_args+=(--unsigned-layout)
if [[ "$SKIP_BUILD" == "1" ]]; then
  package_args+=(--skip-build)
fi

mkdir -p "$DIST_DIR"
log "building macOS release archive"
"$ROOT_DIR/scripts/package-release.sh" "${package_args[@]}"

archive="$(
  find "$DIST_DIR" -maxdepth 1 -type f -name "mayhem-$VERSION-*-apple-darwin.tar.gz" \
    | sort \
    | head -n 1
)"
[[ -n "$archive" ]] || die "macOS archive was not created in $DIST_DIR"
[[ -f "$archive.sha256" ]] || die "checksum sidecar missing: $archive.sha256"

WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-macos-opencode-check.XXXXXX")"
home_dir="$WORK_ROOT/home"
install_dir="$WORK_ROOT/bin"
npm_prefix="$WORK_ROOT/npm"
install_out="$WORK_ROOT/install.out"
gateway_log="$WORK_ROOT/gateway.log"
mkdir -p "$home_dir" "$install_dir" "$npm_prefix"

log "installing $(basename "$archive") into temporary HOME"
HOME="$home_dir" \
MAYHEM_NPM_PREFIX="$npm_prefix" \
"$ROOT_DIR/install.sh" \
  --artifact "$archive" \
  --unsigned-layout \
  --install-dir "$install_dir" \
  --force-opencode \
  --no-path-update \
  >"$install_out" 2>&1

cat "$install_out"
grep -F "verified archive SHA-256" "$install_out" >/dev/null
grep -F "packaged file checksum(s)" "$install_out" >/dev/null
grep -F "installed opencode v1.17.13" "$install_out" >/dev/null
grep -F "Copy/paste PATH for this shell session:" "$install_out" >/dev/null

test -x "$install_dir/mayhem"
test -x "$install_dir/mayhem-gateway"
test -x "$install_dir/opencode"

gateway_port="${MAYHEM_MACOS_GATEWAY_PORT:-$(free_port)}"
gateway_url="http://127.0.0.1:$gateway_port"

log "starting installed gateway on $gateway_url"
"$install_dir/mayhem-gateway" \
  --dev-embedded-catalog \
  --bind "127.0.0.1:$gateway_port" \
  >"$gateway_log" 2>&1 &
GATEWAY_PID=$!

for _ in $(seq 1 60); do
  if curl -fsS "$gateway_url/mayhem/status" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$GATEWAY_PID" >/dev/null 2>&1; then
    cat "$gateway_log" >&2 || true
    die "gateway exited before becoming healthy"
  fi
  sleep 1
done
curl -fsS "$gateway_url/mayhem/status" >/dev/null

log "running provider opencode role smoke"
run_role provider "$gateway_url" "$install_dir"

log "running user opencode role smoke"
run_role user "$gateway_url" "$install_dir"

log "running fresh opencode config merge check"
run_fresh_config_check "$gateway_url" "$install_dir"

if [[ "${MAYHEM_MACOS_OPENCODE_KEEP_TMP:-0}" == "1" ]]; then
  log "kept temporary install root at $WORK_ROOT"
fi

log "macOS local opencode role check passed"
