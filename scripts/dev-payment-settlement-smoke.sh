#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$ROOT_DIR/intercom"
MAYHEM_BIN="$ROOT_DIR/target/debug/mayhem"
GATEWAY_BIN="$ROOT_DIR/target/debug/mayhem-gateway"
MSB_HELPER="$ROOT_DIR/crates/mayhem-cli/src/msb-transfer-helper.mjs"
GATEWAY_BIND="${MAYHEM_PAYMENT_SMOKE_GATEWAY_BIND:-127.0.0.1:11439}"
GATEWAY_URL="http://$GATEWAY_BIND"
EPOCH="${MAYHEM_PAYMENT_SMOKE_EPOCH:-1}"
EPOCH_AT="${MAYHEM_PAYMENT_SMOKE_EPOCH_AT:-90000}"
PAYOUT_EPOCH="${MAYHEM_PAYMENT_SMOKE_PAYOUT_EPOCH:-2}"
PAYOUT_AT="${MAYHEM_PAYMENT_SMOKE_PAYOUT_AT:-93600}"
FEE_BPS="${MAYHEM_PAYMENT_SMOKE_FEE_BPS:-5000}"
PAYOUT_RATE_E6="${MAYHEM_PAYMENT_SMOKE_PAYOUT_RATE_E6:-1000}"
SEED_MU="${MAYHEM_PAYMENT_SMOKE_SEED_MU:-1000000}"
TREASURY_ADDRESS="${MAYHEM_PAYMENT_SMOKE_TREASURY_ADDRESS:-testtrac1lvh6wsglmzly5gmdck79j5asjp6fgr7dsuhsew3fvgcxfeu83acs0zvcsh}"
FUNDED_STORES_DIRECTORY="${MAYHEM_PAYMENT_SMOKE_FUNDED_STORES_DIRECTORY:-$ROOT_DIR/.mayhem-local}"
FUNDED_STORE_NAME="${MAYHEM_PAYMENT_SMOKE_FUNDED_STORE_NAME:-testnet-epoch-wallet}"
KEEP_TMP="${MAYHEM_PAYMENT_SMOKE_KEEP_TMP:-0}"

TMP_DIR=""
DEVNET_PID=""
GATEWAY_PID=""
COLLECT_PID=""
DEVNET_PEER_PIDS=""

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/dev-payment-settlement-smoke.sh

Runs the active payment settlement acceptance path on a fresh local Intercom
dev-net:
  - admin creates params/model/enclave/price/room records;
  - a deterministic dev provider wallet consents and joins admin records;
  - gateway receipts are published and collected over mx/epoch/<epoch>;
  - an independent recompute output drives epochCommit and epochApply;
  - provider payout and router fee sweep are executed as real public testnet
    MSB transfers and confirmed with payoutConfirm.

Environment:
  MAYHEM_PAYMENT_SMOKE_KEEP_TMP=1 keeps temporary evidence files.
  MAYHEM_PAYMENT_SMOKE_* overrides epoch, bind, rate, treasury, and wallet paths.
USAGE
}

cleanup() {
  if [[ -n "$COLLECT_PID" ]] && kill -0 "$COLLECT_PID" 2>/dev/null; then
    kill "$COLLECT_PID" 2>/dev/null || true
    wait "$COLLECT_PID" 2>/dev/null || true
  fi
  if [[ -n "$GATEWAY_PID" ]] && kill -0 "$GATEWAY_PID" 2>/dev/null; then
    kill "$GATEWAY_PID" 2>/dev/null || true
    wait "$GATEWAY_PID" 2>/dev/null || true
  fi
  if [[ -n "$DEVNET_PEER_PIDS" ]]; then
    # shellcheck disable=SC2086
    kill $DEVNET_PEER_PIDS 2>/dev/null || true
    # shellcheck disable=SC2086
    wait $DEVNET_PEER_PIDS 2>/dev/null || true
  fi
  if [[ -n "$DEVNET_PID" ]] && kill -0 "$DEVNET_PID" 2>/dev/null; then
    kill "$DEVNET_PID" 2>/dev/null || true
    wait "$DEVNET_PID" 2>/dev/null || true
  fi
  if [[ -n "$TMP_DIR" && "$KEEP_TMP" != "1" ]]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT INT TERM

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v cargo >/dev/null 2>&1 || die "cargo is required"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v node >/dev/null 2>&1 || die "node is required"

if ! [[ "$EPOCH" =~ ^[1-9][0-9]*$ ]]; then
  die "MAYHEM_PAYMENT_SMOKE_EPOCH must be positive"
fi
if ! [[ "$PAYOUT_EPOCH" =~ ^[1-9][0-9]*$ ]]; then
  die "MAYHEM_PAYMENT_SMOKE_PAYOUT_EPOCH must be positive"
fi
if ! [[ "$FEE_BPS" =~ ^[0-9]+$ ]] || (( FEE_BPS > 10000 )); then
  die "MAYHEM_PAYMENT_SMOKE_FEE_BPS must be 0..10000"
fi
if [[ ! -f "$FUNDED_STORES_DIRECTORY/$FUNDED_STORE_NAME/db/keypair.json" ]]; then
  die "funded testnet wallet missing at $FUNDED_STORES_DIRECTORY/$FUNDED_STORE_NAME/db/keypair.json"
fi
if curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1; then
  die "$GATEWAY_URL already has a Mayhem gateway; set MAYHEM_PAYMENT_SMOKE_GATEWAY_BIND"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-payment-settlement.XXXXXX")"
DEVNET_LOG="$TMP_DIR/dev-net.log"
GATEWAY_LOG="$TMP_DIR/gateway.log"
TEST_JSON="$TMP_DIR/test.json"
GATEWAY_RECEIPTS_JSON="$TMP_DIR/gateway-receipts.json"
COLLECTED_JSON="$TMP_DIR/collected-receipts.json"
COLLECT_REPORT_JSON="$TMP_DIR/collect-report.json"
PUBLISH_JSON="$TMP_DIR/publish.json"
META_JSON="$TMP_DIR/receipt-meta.json"
PROVIDER_HOME="$TMP_DIR/provider-home"
ADMIN_HOME="$TMP_DIR/admin-home"
PROVIDER_WALLET_JSON="$TMP_DIR/provider-wallet.json"
DEPOSITS_JSON="$TMP_DIR/deposits.json"
BUNDLE_JSON="$TMP_DIR/bundle.json"
EXPORT_UNVERIFIED_JSON="$TMP_DIR/export-unverified.json"
RECOMPUTED_JSON="$TMP_DIR/recomputed.json"
COMMIT_JSON="$TMP_DIR/epoch-commit.json"
APPLY_JSON="$TMP_DIR/epoch-apply.json"
EXPORT_VERIFIED_JSON="$TMP_DIR/export-verified.json"
RATE_JSON="$TMP_DIR/payout-rate.json"
PAYOUT_PLAN_JSON="$TMP_DIR/payout-plan.json"
PROVIDER_MSB_JSON="$TMP_DIR/provider-msb.json"
FEE_MSB_JSON="$TMP_DIR/fee-msb.json"
PROVIDER_PAYOUT_JSON="$TMP_DIR/provider-payout-confirm.json"
FEE_SWEEP_JSON="$TMP_DIR/fee-sweep-confirm.json"
PAYOUTS_JSON="$TMP_DIR/payouts.json"
SET_RULES_JSON="$TMP_DIR/admin-set-rules.json"
SET_PARAMS_JSON="$TMP_DIR/admin-set-params.json"
SET_MODEL_REF_JSON="$TMP_DIR/admin-set-model-ref.json"
REGISTER_ENCLAVE_JSON="$TMP_DIR/admin-register-enclave.json"
SET_PRICE_JSON="$TMP_DIR/admin-set-price.json"
OPEN_ROOM_JSON="$TMP_DIR/admin-open-room.json"
SET_PROVIDER_PAYOUT_JSON="$TMP_DIR/admin-set-provider-payout.json"
PROVIDER_SETUP_JSON="$TMP_DIR/provider-setup.json"
PROVIDER_JOIN_JSON="$TMP_DIR/provider-join.json"

log "building mayhem CLI and gateway"
(cd "$ROOT_DIR" && cargo build -q -p mayhem-cli -p mayhem-gateway)

log "starting fresh local Intercom dev-net"
(
  cd "$ROOT_DIR"
  MAYHEM_DEVNET_JOINERS=1 scripts/dev-net.sh --cleanup --keep-running
) >"$DEVNET_LOG" 2>&1 &
DEVNET_PID="$!"

for _ in $(seq 1 720); do
  if grep -q "Mayhem dev-net ready." "$DEVNET_LOG" 2>/dev/null; then
    break
  fi
  if ! kill -0 "$DEVNET_PID" 2>/dev/null; then
    sed -n '1,160p' "$DEVNET_LOG" >&2 || true
    die "dev-net exited before becoming ready"
  fi
  sleep 0.5
done
grep -q "Mayhem dev-net ready." "$DEVNET_LOG" || die "timed out waiting for dev-net"

ADMIN_WS="$(awk '/admin:/ { print $2; exit }' "$DEVNET_LOG")"
JOINER_WS="$(awk '/joiner-a:/ { print $2; exit }' "$DEVNET_LOG")"
ADMIN_RPC="$(awk '/admin:/ { for (i = 1; i <= NF; i++) if ($i ~ /^rpc=/) { sub(/^rpc=/, "", $i); print $i; exit } }' "$DEVNET_LOG")"
TOKEN="$(awk -F':  +' '/sc bridge token:/ { print $2; exit }' "$DEVNET_LOG")"
DEVNET_PEER_PIDS="$(sed -n 's/^Peers are still running\. Stop them with: kill //p' "$DEVNET_LOG" | tail -1)"

[[ -n "$ADMIN_WS" && -n "$JOINER_WS" && -n "$ADMIN_RPC" && -n "$TOKEN" ]] \
  || die "could not parse dev-net endpoints"

log "starting development gateway on $GATEWAY_URL"
"$GATEWAY_BIN" --dev-embedded-catalog --bind "$GATEWAY_BIND" >"$GATEWAY_LOG" 2>&1 &
GATEWAY_PID="$!"
for _ in $(seq 1 160); do
  if curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
    sed -n '1,160p' "$GATEWAY_LOG" >&2 || true
    die "gateway exited before becoming ready"
  fi
  sleep 0.25
done
curl -fsS "$GATEWAY_URL/mayhem/status" >/dev/null 2>&1 \
  || die "timed out waiting for gateway"

log "generating gateway receipts"
"$MAYHEM_BIN" test \
  --gateway-url "$GATEWAY_URL" \
  --skip-peer-health \
  --skip-opencode \
  --json >"$TEST_JSON"
curl -fsS "$GATEWAY_URL/mayhem/receipts" >"$GATEWAY_RECEIPTS_JSON"

node - "$GATEWAY_RECEIPTS_JSON" "$META_JSON" <<'NODE'
const fs = require('node:fs');
const [receiptsPath, metaPath] = process.argv.slice(2);
const root = JSON.parse(fs.readFileSync(receiptsPath, 'utf8'));
const receipts = Array.isArray(root.data) ? root.data : [];
if (receipts.length < 1) throw new Error('gateway produced no receipts');
const bodies = receipts.map((entry) => {
  const receipt = entry.receipt ?? entry;
  return receipt.body ?? receipt;
});
const first = bodies[0];
for (const field of ['user', 'provider', 'enclave_id', 'model_id']) {
  if (typeof first[field] !== 'string' || first[field].length === 0) {
    throw new Error(`receipt missing ${field}`);
  }
}
for (const body of bodies) {
  for (const field of ['user', 'provider', 'enclave_id', 'model_id']) {
    if (body[field] !== first[field]) throw new Error(`mixed receipt ${field} values`);
  }
}
fs.writeFileSync(metaPath, JSON.stringify({
  count: receipts.length,
  user: first.user,
  provider: first.provider,
  enclave_id: first.enclave_id,
  model_id: first.model_id,
}, null, 2) + '\n');
NODE

RECEIPT_COUNT="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).count' "$META_JSON")"
USER_PUBKEY="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).user' "$META_JSON")"
PROVIDER_PUBKEY="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).provider' "$META_JSON")"
ENCLAVE_ID="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).enclave_id' "$META_JSON")"
MODEL_ID="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).model_id' "$META_JSON")"

log "creating deterministic provider wallet for receipt provider"
mkdir -p "$PROVIDER_HOME/stores/main/db"
node --input-type=module - "$ROOT_DIR" "$PROVIDER_HOME/stores/main/db/keypair.json" "$PROVIDER_WALLET_JSON" "$PROVIDER_PUBKEY" <<'NODE'
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
const [rootDir, keypairPath, outPath, expectedProvider] = process.argv.slice(2);
const require = createRequire(import.meta.url);
const crypto = require(path.join(rootDir, 'intercom/node_modules/hypercore-crypto'));
const { default: Wallet } = await import(pathToFileURL(path.join(rootDir, 'intercom/node_modules/trac-wallet/index.js')).href);
const tracCryptoApi = (await import(pathToFileURL(path.join(rootDir, 'intercom/node_modules/trac-crypto-api/index.js')).href)).default;
const seed = Buffer.alloc(32, 42);
const wallet = await Wallet.fromKeyPair(crypto.keyPair(seed));
if (Buffer.from(wallet.publicKey).toString('hex') !== expectedProvider) {
  throw new Error('deterministic provider wallet does not match receipt provider');
}
await wallet.exportToFile(keypairPath, Buffer.alloc(0));
fs.writeFileSync(outPath, JSON.stringify({
  public_key: Buffer.from(wallet.publicKey).toString('hex'),
  mainnet_address: wallet.address,
  testnet_address: tracCryptoApi.address.encode('testtrac', wallet.publicKey),
}, null, 2) + '\n');
NODE
PROVIDER_TESTTRAC="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).testnet_address' "$PROVIDER_WALLET_JSON")"

RULES_HASH="$("$MAYHEM_BIN" rules hash --print-json | node -pe 'JSON.parse(require("fs").readFileSync(0,"utf8")).hash')"
mkdir -p "$ADMIN_HOME/stores"
ln -s "$APP_DIR/stores/mayhem-devnet-admin" "$ADMIN_HOME/stores/admin"
ADMIN_COMMON=(--home "$ADMIN_HOME" --peer-store-name admin --rpc-url "$ADMIN_RPC" --submit --json)

log "seeding admin control plane"
"$MAYHEM_BIN" admin set-rules "${ADMIN_COMMON[@]}" --ver 1 --hash "$RULES_HASH" >"$SET_RULES_JSON"
"$MAYHEM_BIN" admin set-params "${ADMIN_COMMON[@]}" \
  --submitted-at 0 \
  --effective-at 86400 \
  --values-json "{\"fee_bps\":$FEE_BPS,\"holdback_epochs\":0,\"challenge_epochs\":0,\"payout_min_mu\":0,\"rate_staleness_seconds\":86400}" >"$SET_PARAMS_JSON"
"$MAYHEM_BIN" admin set-model-ref "${ADMIN_COMMON[@]}" \
  --model "$MODEL_ID" \
  --in-per-1k-mu 20 \
  --out-per-1k-mu 60 >"$SET_MODEL_REF_JSON"
"$MAYHEM_BIN" admin register-enclave "${ADMIN_COMMON[@]}" \
  --enclave-id "$ENCLAVE_ID" \
  --model "$MODEL_ID" \
  --backend llama.cpp \
  --artifact-root "$(printf '1%.0s' {1..64})" \
  --artifact-root-kind blake3_merkle_v1 \
  --artifact-repo mayhem-smoke/dev-payment \
  --artifact-revision "$(printf '4%.0s' {1..40})" \
  --artifact-path dev-payment-smoke.gguf \
  --manifest-hash "$(printf '2%.0s' {1..64})" \
  --binary-hash "$(printf '3%.0s' {1..64})" \
  --caps-json '{"chat":true,"tools":true,"json":true,"ctx":8192}' >"$REGISTER_ENCLAVE_JSON"
"$MAYHEM_BIN" admin set-price "${ADMIN_COMMON[@]}" \
  --enclave-id "$ENCLAVE_ID" \
  --in-per-1k-mu 20 \
  --out-per-1k-mu 60 \
  --effective-at "$EPOCH_AT" >"$SET_PRICE_JSON"
"$MAYHEM_BIN" admin open-room "${ADMIN_COMMON[@]}" \
  --enclave-id "$ENCLAVE_ID" \
  --model "$MODEL_ID" \
  --nonce p5-payment-settlement-smoke \
  --label dev-payment-settlement >"$OPEN_ROOM_JSON"

log "consenting and joining deterministic provider"
"$MAYHEM_BIN" setup \
  --home "$PROVIDER_HOME" \
  --role provider \
  --wallet reuse \
  --peer-store-name main \
  --rpc-url "$ADMIN_RPC" \
  --rules-ver 1 \
  --rules-hash "$RULES_HASH" \
  --yes \
  --print-json >"$PROVIDER_SETUP_JSON"
"$MAYHEM_BIN" provider join \
  --home "$PROVIDER_HOME" \
  --peer-store-name main \
  --rpc-url "$ADMIN_RPC" \
  --enclave "$ENCLAVE_ID" \
  --rooms auto \
  --json >"$PROVIDER_JOIN_JSON"
"$MAYHEM_BIN" admin set-provider-payout "${ADMIN_COMMON[@]}" \
  --provider "$PROVIDER_PUBKEY" \
  --payout-method tnk \
  --payout-addr "$PROVIDER_TESTTRAC" >"$SET_PROVIDER_PAYOUT_JSON"

log "publishing and collecting receipts over mx/epoch/$EPOCH"
"$MAYHEM_BIN" receipts collect \
  --epoch "$EPOCH" \
  --sc-bridge-url "$JOINER_WS" \
  --sc-bridge-token "$TOKEN" \
  --max-receipts "$RECEIPT_COUNT" \
  --timeout-seconds 45 \
  --output "$COLLECTED_JSON" \
  --json >"$COLLECT_REPORT_JSON" &
COLLECT_PID="$!"
sleep 2
"$MAYHEM_BIN" receipts publish \
  --epoch "$EPOCH" \
  --gateway-url "$GATEWAY_URL" \
  --sc-bridge-url "$ADMIN_WS" \
  --sc-bridge-token "$TOKEN" \
  --max-receipts "$RECEIPT_COUNT" \
  --json >"$PUBLISH_JSON"
wait "$COLLECT_PID"
COLLECT_PID=""

node - "$COLLECTED_JSON" "$RECEIPT_COUNT" <<'NODE'
const fs = require('node:fs');
const [path, expectedRaw] = process.argv.slice(2);
const expected = Number(expectedRaw);
const value = JSON.parse(fs.readFileSync(path, 'utf8'));
if (!Array.isArray(value.data) || value.data.length !== expected) {
  throw new Error(`collected ${value.data?.length ?? 0} receipts, expected ${expected}`);
}
NODE

log "seeding user credit and recomputing epoch roots"
EXT_REF_HASH="$(node -e 'const crypto=require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update("mayhem-dev-payment-settlement-smoke").digest("hex"))')"
"$MAYHEM_BIN" admin fiat-deposit "${ADMIN_COMMON[@]}" \
  --rail stripe \
  --who "$USER_PUBKEY" \
  --mu "$SEED_MU" \
  --ext-ref-hash "$EXT_REF_HASH" \
  --epoch "$EPOCH" \
  --at "$EPOCH_AT" >/dev/null

node --input-type=module - "$ROOT_DIR" "$DEPOSITS_JSON" "$USER_PUBKEY" "$SEED_MU" "$EXT_REF_HASH" <<'NODE'
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
const [rootDir, depositsPath, user, seedMuRaw, extRefHash] = process.argv.slice(2);
const { opaqueHash } = await import(pathToFileURL(path.join(rootDir, 'intercom/scripts/recompute-epoch-roots.mjs')).href);
fs.writeFileSync(depositsPath, JSON.stringify([{
  rail: 'stripe',
  user_hash: await opaqueHash('deposit-user', user),
  mu: Number(seedMuRaw),
  ext_ref_hash: extRefHash,
}], null, 2) + '\n');
NODE

"$MAYHEM_BIN" receipts export \
  --epoch "$EPOCH" \
  --fee-bps "$FEE_BPS" \
  --receipts-file "$COLLECTED_JSON" \
  --deposits-file "$DEPOSITS_JSON" \
  --output "$BUNDLE_JSON" \
  --no-verify \
  --json >"$EXPORT_UNVERIFIED_JSON"
node "$ROOT_DIR/intercom/scripts/recompute-epoch-roots.mjs" "$BUNDLE_JSON" >"$RECOMPUTED_JSON"

log "submitting epochCommit and epochApply"
"$MAYHEM_BIN" admin epoch-commit "${ADMIN_COMMON[@]}" \
  --epoch "$EPOCH" \
  --at "$EPOCH_AT" \
  --recomputed-file "$RECOMPUTED_JSON" >"$COMMIT_JSON"
"$MAYHEM_BIN" admin epoch-apply "${ADMIN_COMMON[@]}" \
  --epoch "$EPOCH" \
  --at "$EPOCH_AT" \
  --recomputed-file "$RECOMPUTED_JSON" >"$APPLY_JSON"

log "verifying exported receipt bundle against live ev/* records"
"$MAYHEM_BIN" receipts export \
  --epoch "$EPOCH" \
  --fee-bps "$FEE_BPS" \
  --receipts-file "$COLLECTED_JSON" \
  --deposits-file "$DEPOSITS_JSON" \
  --rpc-url "$ADMIN_RPC" \
  --output "$TMP_DIR/verified-bundle.json" \
  --json >"$EXPORT_VERIFIED_JSON"

log "executing TNK provider payout and fee sweep on public testnet MSB"
"$MAYHEM_BIN" admin rate-oracle "${ADMIN_COMMON[@]}" \
  --tnk-usd-e6 "$PAYOUT_RATE_E6" \
  --source gate-spot \
  --ts "$PAYOUT_AT" >"$RATE_JSON"

node - "$RECOMPUTED_JSON" "$PAYOUT_PLAN_JSON" "$PAYOUT_RATE_E6" <<'NODE'
const fs = require('node:fs');
const [recomputedPath, outPath, rateRaw] = process.argv.slice(2);
const r = JSON.parse(fs.readFileSync(recomputedPath, 'utf8'));
const rate = BigInt(rateRaw);
const feeBps = BigInt(r.params.fee_bps);
if (!Array.isArray(r.earnings) || r.earnings.length !== 1) {
  throw new Error('smoke expects exactly one provider earning');
}
const gross = BigInt(r.earnings[0].gross_mu);
const feeMu = Number((gross * feeBps) / 10000n);
const providerMu = Number(gross - BigInt(feeMu));
if (providerMu <= 0) throw new Error(`provider payout amount must be positive, got ${providerMu}`);
if (feeMu <= 0) throw new Error(`fee sweep amount must be positive, got ${feeMu}`);
const e18 = 1000000000000000000n;
const ceilDiv = (a, b) => (a + b - 1n) / b;
const decimal = (value) => {
  const whole = value / e18;
  let frac = String(value % e18).padStart(18, '0').replace(/0+$/, '');
  return frac.length ? `${whole}.${frac}` : `${whole}`;
};
const toTnk = (mu) => ceilDiv(BigInt(mu) * e18, rate);
const providerTnk = toTnk(providerMu);
const feeTnk = toTnk(feeMu);
fs.writeFileSync(outPath, JSON.stringify({
  provider_mu: providerMu,
  fee_mu: feeMu,
  provider_tnk_e18: providerTnk.toString(),
  fee_tnk_e18: feeTnk.toString(),
  provider_tnk_amount: decimal(providerTnk),
  fee_tnk_amount: decimal(feeTnk),
}, null, 2) + '\n');
NODE

PROVIDER_MU="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).provider_mu' "$PAYOUT_PLAN_JSON")"
FEE_MU="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).fee_mu' "$PAYOUT_PLAN_JSON")"
PROVIDER_TNK_E18="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).provider_tnk_e18' "$PAYOUT_PLAN_JSON")"
FEE_TNK_E18="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).fee_tnk_e18' "$PAYOUT_PLAN_JSON")"
PROVIDER_TNK_AMOUNT="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).provider_tnk_amount' "$PAYOUT_PLAN_JSON")"
FEE_TNK_AMOUNT="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).fee_tnk_amount' "$PAYOUT_PLAN_JSON")"

node "$MSB_HELPER" transfer \
  --network testnet1 \
  --stores-directory "$FUNDED_STORES_DIRECTORY" \
  --store-name "$FUNDED_STORE_NAME" \
  --to "$PROVIDER_TESTTRAC" \
  --amount "$PROVIDER_TNK_AMOUNT" \
  --timeout-seconds 240 >"$PROVIDER_MSB_JSON"
node "$MSB_HELPER" transfer \
  --network testnet1 \
  --stores-directory "$FUNDED_STORES_DIRECTORY" \
  --store-name "$FUNDED_STORE_NAME" \
  --to "$TREASURY_ADDRESS" \
  --amount "$FEE_TNK_AMOUNT" \
  --timeout-seconds 240 >"$FEE_MSB_JSON"

PROVIDER_MSB_TX="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).tx_hash' "$PROVIDER_MSB_JSON")"
FEE_MSB_TX="$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).tx_hash' "$FEE_MSB_JSON")"

"$MAYHEM_BIN" admin payout-confirm "${ADMIN_COMMON[@]}" \
  --kind provider \
  --rail tnk \
  --epoch "$PAYOUT_EPOCH" \
  --who "$PROVIDER_PUBKEY" \
  --mu "$PROVIDER_MU" \
  --tnk-e18 "$PROVIDER_TNK_E18" \
  --msb-tx-hash "$PROVIDER_MSB_TX" \
  --at "$PAYOUT_AT" >"$PROVIDER_PAYOUT_JSON"
"$MAYHEM_BIN" admin payout-confirm "${ADMIN_COMMON[@]}" \
  --kind fee-sweep \
  --rail tnk \
  --epoch "$EPOCH" \
  --who treasury \
  --mu "$FEE_MU" \
  --tnk-e18 "$FEE_TNK_E18" \
  --msb-tx-hash "$FEE_MSB_TX" \
  --at "$PAYOUT_AT" >"$FEE_SWEEP_JSON"

"$MAYHEM_BIN" payouts --epoch "$PAYOUT_EPOCH" --rpc-url "$ADMIN_RPC" --json >"$PAYOUTS_JSON"
"$MAYHEM_BIN" payouts --epoch "$EPOCH" --rpc-url "$ADMIN_RPC" --json >"$TMP_DIR/fee-payouts.json"

node - "$EXPORT_VERIFIED_JSON" "$PROVIDER_PAYOUT_JSON" "$FEE_SWEEP_JSON" "$PAYOUTS_JSON" "$TMP_DIR/fee-payouts.json" <<'NODE'
const fs = require('node:fs');
const [exportPath, providerPath, feePath, payoutsPath, feePayoutsPath] = process.argv.slice(2);
const exp = JSON.parse(fs.readFileSync(exportPath, 'utf8'));
if (exp.verified !== true) throw new Error('receipt export did not verify against live ev/*');
const provider = JSON.parse(fs.readFileSync(providerPath, 'utf8'));
const fee = JSON.parse(fs.readFileSync(feePath, 'utf8'));
if (provider.tx_type !== 'payoutConfirm' || provider.tx?.result?.local !== true) {
  throw new Error('provider payoutConfirm was not submitted');
}
if (fee.tx_type !== 'payoutConfirm' || fee.tx?.result?.local !== true) {
  throw new Error('fee sweep payoutConfirm was not submitted');
}
const payouts = JSON.parse(fs.readFileSync(payoutsPath, 'utf8'));
if (!payouts.pay || payouts.pay.count < 1 || payouts.pay.mu_total < 1) {
  throw new Error('provider payout evidence missing');
}
const feePayouts = JSON.parse(fs.readFileSync(feePayoutsPath, 'utf8'));
if (!feePayouts.fee || feePayouts.fee.sweep_mu < 1 || !feePayouts.fee.sweep_msb_tx_hash) {
  throw new Error('fee sweep evidence missing');
}
NODE

node - "$META_JSON" "$RECOMPUTED_JSON" "$PAYOUT_PLAN_JSON" "$PROVIDER_MSB_JSON" "$FEE_MSB_JSON" "$EXPORT_VERIFIED_JSON" <<'NODE'
const fs = require('node:fs');
const [metaPath, recomputedPath, payoutPlanPath, providerMsbPath, feeMsbPath, exportPath] = process.argv.slice(2);
const meta = JSON.parse(fs.readFileSync(metaPath, 'utf8'));
const recomputed = JSON.parse(fs.readFileSync(recomputedPath, 'utf8'));
const payout = JSON.parse(fs.readFileSync(payoutPlanPath, 'utf8'));
const providerMsb = JSON.parse(fs.readFileSync(providerMsbPath, 'utf8'));
const feeMsb = JSON.parse(fs.readFileSync(feeMsbPath, 'utf8'));
const verified = JSON.parse(fs.readFileSync(exportPath, 'utf8'));
console.log(JSON.stringify({
  ok: true,
  epoch: recomputed.epoch,
  receipts_collected: meta.count,
  use_mu: recomputed.totals.use_mu,
  earn_mu: recomputed.totals.earn_mu,
  fee_mu: recomputed.totals.fee_mu,
  provider_mu: payout.provider_mu,
  provider_msb_tx: providerMsb.tx_hash,
  fee_msb_tx: feeMsb.tx_hash,
  checks: verified.checks.length,
}, null, 2));
NODE

log "payment settlement smoke passed"
if [[ "$KEEP_TMP" == "1" ]]; then
  log "kept temporary files in $TMP_DIR"
fi
