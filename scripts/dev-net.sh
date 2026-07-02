#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/dev-net.sh [--cleanup] [--keep-running]

Boots a local Intercom dev net with one admin peer and two joiners.

Options:
  --cleanup       Remove the dev-net stores before starting. Safe to repeat.
  --keep-running  Leave peers running after verification.
USAGE
}

cleanup_stores=0
keep_running=0
for arg in "$@"; do
  case "$arg" in
    --cleanup)
      cleanup_stores=1
      ;;
    --keep-running)
      keep_running=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_dir="$repo_root/intercom"
stores_dir="$app_dir/stores"
log_dir="$stores_dir/dev-net-logs"
pear_runtime="${MAYHEM_PEAR_RUNTIME:-$HOME/Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime}"

if [[ ! -x "$pear_runtime" ]]; then
  echo "pear-runtime not found. Set MAYHEM_PEAR_RUNTIME to the runtime binary." >&2
  exit 1
fi

if [[ ! -d "$app_dir" ]]; then
  echo "missing Mayhem Intercom app at $app_dir" >&2
  exit 1
fi

if [[ "$cleanup_stores" -eq 1 ]]; then
  rm -rf \
    "$stores_dir/mayhem-devnet-admin" \
    "$stores_dir/mayhem-devnet-admin-msb" \
    "$stores_dir/mayhem-devnet-joiner-a" \
    "$stores_dir/mayhem-devnet-joiner-a-msb" \
    "$stores_dir/mayhem-devnet-joiner-b" \
    "$stores_dir/mayhem-devnet-joiner-b-msb" \
    "$log_dir"
fi
mkdir -p "$log_dir"

token="mayhem-devnet-token-$(date +%s)-$$"
subnet_channel="mayhem-devnet-local"

pids=()
cleanup_processes() {
  if [[ "$keep_running" -eq 1 ]]; then
    return
  fi
  for pid in "${pids[@]:-}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  for pid in "${pids[@]:-}"; do
    wait "$pid" 2>/dev/null || true
  done
}
trap cleanup_processes EXIT INT TERM

free_port() {
  node -e 'const net = require("net"); const s = net.createServer(); s.listen(0, "127.0.0.1", () => { console.log(s.address().port); s.close(); });'
}

admin_port="$(free_port)"
joiner_a_port="$(free_port)"
joiner_b_port="$(free_port)"

start_peer() {
  local label="$1"
  local store="$2"
  local msb_store="$3"
  local port="$4"
  local bootstrap="${5:-}"
  local log_file="$log_dir/$label.log"

  local args=(
    run
    .
    --peer-store-name "$store"
    --msb-store-name "$msb_store"
    --subnet-channel "$subnet_channel"
    --sidechannel-quiet 1
    --sc-bridge 1
    --sc-bridge-host 127.0.0.1
    --sc-bridge-port "$port"
    --sc-bridge-token "$token"
    --sc-bridge-cli 1
  )
  if [[ -n "$bootstrap" ]]; then
    args+=(--subnet-bootstrap "$bootstrap")
  fi

  (
    cd "$app_dir"
    "$pear_runtime" "${args[@]}"
  ) >"$log_file" 2>&1 &
  local pid="$!"
  pids+=("$pid")
  echo "$pid"
}

wait_log_contains() {
  local log_file="$1"
  local needle="$2"
  local timeout_sec="$3"
  local deadline=$((SECONDS + timeout_sec))
  while (( SECONDS < deadline )); do
    if grep -qF "$needle" "$log_file" 2>/dev/null; then
      return 0
    fi
    if [[ -f "$log_file" ]]; then
      for pid in "${pids[@]:-}"; do
        if ! kill -0 "$pid" 2>/dev/null; then
          echo "peer exited while waiting for '$needle'. See $log_file" >&2
          return 1
        fi
      done
    fi
    sleep 0.25
  done
  echo "timed out waiting for '$needle' in $log_file" >&2
  return 1
}

wait_log_value() {
  local log_file="$1"
  local label="$2"
  local timeout_sec="$3"
  local deadline=$((SECONDS + timeout_sec))
  while (( SECONDS < deadline )); do
    if [[ -f "$log_file" ]]; then
      local value
      value="$(
        awk -v label="$label" 'index($0, label) { value = substr($0, index($0, label) + length(label)); gsub(/^[[:space:]]+|[[:space:]]+$/, "", value); print value; exit }' "$log_file"
      )"
      if [[ -n "$value" ]]; then
        printf '%s\n' "$value"
        return 0
      fi
    fi
    sleep 0.25
  done
  echo "timed out waiting for '$label' in $log_file" >&2
  return 1
}

bridge_request() {
  local port="$1"
  local payload="$2"
  node - "$port" "$token" "$payload" <<'NODE'
const [port, token, rawPayload] = process.argv.slice(2);
const payload = JSON.parse(rawPayload);
const ws = new WebSocket(`ws://127.0.0.1:${port}`);
let sent = false;
const timer = setTimeout(() => {
  console.error(`SC-Bridge request timed out on ${port}`);
  process.exit(1);
}, 10000);

function finish(code, value) {
  clearTimeout(timer);
  if (value !== undefined) {
    const out = typeof value === 'string' ? value : JSON.stringify(value);
    if (code === 0) console.log(out);
    else console.error(out);
  }
  try { ws.close(); } catch {}
  process.exit(code);
}

ws.onerror = (error) => finish(1, error?.message || String(error));
ws.onopen = () => ws.send(JSON.stringify({ id: 1, type: 'auth', token }));
ws.onmessage = (event) => {
  let msg;
  try {
    msg = JSON.parse(event.data);
  } catch {
    return;
  }
  if (msg.id === 1 && msg.type === 'auth_ok') {
    payload.id = 2;
    sent = true;
    ws.send(JSON.stringify(payload));
    return;
  }
  if (msg.id === 1 && msg.type === 'error') {
    finish(1, msg);
    return;
  }
  if (sent && msg.id === 2) {
    if (msg.type === 'error') finish(1, msg);
    else finish(0, msg);
  }
};
NODE
}

wait_bridge_connections() {
  local label="$1"
  local port="$2"
  local min_connections="$3"
  local timeout_sec="$4"
  local deadline=$((SECONDS + timeout_sec))
  local stats='{}'
  while (( SECONDS < deadline )); do
    stats="$(bridge_request "$port" '{"type":"stats"}' 2>/dev/null || true)"
    if node -e 'const stats = JSON.parse(process.argv[1] || "{}"); process.exit((stats.connectionCount || 0) >= Number(process.argv[2]) ? 0 : 1)' "$stats" "$min_connections"; then
      printf '%s\n' "$stats"
      return 0
    fi
    sleep 1
  done
  echo "$label did not reach $min_connections connections. Last stats: $stats" >&2
  return 1
}

admin_log="$log_dir/admin.log"
joiner_a_log="$log_dir/joiner-a.log"
joiner_b_log="$log_dir/joiner-b.log"

echo "Starting admin peer..."
start_peer "admin" "mayhem-devnet-admin" "mayhem-devnet-admin-msb" "$admin_port" >/dev/null
admin_bootstrap="$(wait_log_value "$admin_log" "Peer subnet bootstrap:" 60)"
admin_pubkey="$(wait_log_value "$admin_log" "Peer pubkey (hex):" 60)"
wait_log_contains "$admin_log" "Sidechannel: ready" 60

echo "Configuring admin and auto-add writers..."
bridge_request "$admin_port" "{\"type\":\"cli\",\"command\":\"/add_admin --address \\\"$admin_pubkey\\\"\"}" >/dev/null
bridge_request "$admin_port" '{"type":"cli","command":"/set_auto_add_writers --enabled 1"}' >/dev/null

echo "Starting joiner peers..."
start_peer "joiner-a" "mayhem-devnet-joiner-a" "mayhem-devnet-joiner-a-msb" "$joiner_a_port" "$admin_bootstrap" >/dev/null
start_peer "joiner-b" "mayhem-devnet-joiner-b" "mayhem-devnet-joiner-b-msb" "$joiner_b_port" "$admin_bootstrap" >/dev/null
wait_log_contains "$joiner_a_log" "Sidechannel: ready" 60
wait_log_contains "$joiner_b_log" "Sidechannel: ready" 60

echo "Waiting for all peers to list both other peers..."
admin_stats="$(wait_bridge_connections admin "$admin_port" 2 90)"
joiner_a_stats="$(wait_bridge_connections joiner-a "$joiner_a_port" 2 90)"
joiner_b_stats="$(wait_bridge_connections joiner-b "$joiner_b_port" 2 90)"

echo "Running simulated Mayhem no-op on admin (--sim 1, no MSB fee)..."
sim_result="$(bridge_request "$admin_port" '{"type":"cli","command":"/tx --command \"noop\" --sim 1"}')"
node -e 'const r = JSON.parse(process.argv[1]); if (r.type !== "cli_result" || r.ok !== true || !r.result || r.result.ok !== true || r.result.op !== "noop") { console.error(process.argv[1]); process.exit(1); }' "$sim_result"

cat <<EOF
Mayhem dev-net ready.
  admin:    ws://127.0.0.1:$admin_port  $admin_stats
  joiner-a: ws://127.0.0.1:$joiner_a_port  $joiner_a_stats
  joiner-b: ws://127.0.0.1:$joiner_b_port  $joiner_b_stats
  subnet bootstrap: $admin_bootstrap
  subnet channel:   $subnet_channel
  logs:             $log_dir
EOF

if [[ "$keep_running" -eq 1 ]]; then
  echo "Peers are still running. Stop them with: kill ${pids[*]}"
  trap - EXIT INT TERM
  wait
fi
