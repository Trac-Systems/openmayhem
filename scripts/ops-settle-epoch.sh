#!/usr/bin/env bash
# OpenMayhem canonical-receipt epoch finalization (admin server).
# Settles the active billing epoch from exact contract-recorded receipt heads:
#   exact sparse snapshot -> recompute -> epoch-commit -> epoch-apply
# Finalization artifacts use a deterministic epoch directory so a retry after
# apply verifies the same frozen evidence instead of attempting the next epoch.
# Every epoch submit is preceded by a sim. Aborts on any mismatch.
# Usage: ops-settle-epoch.sh [epoch]   (defaults to updated_epoch + 1)
set -euo pipefail

umask 077

RPC_URL="${MAYHEM_RPC_URL:-http://127.0.0.1:49223/v1}"
MAYHEM_BIN="${MAYHEM_BIN:-/opt/mayhem/source/target/release/mayhem}"
ADMIN_HOME="${MAYHEM_ADMIN_HOME:-/opt/mayhem/.mayhem-local/live-home}"
ADMIN_STORE="${MAYHEM_ADMIN_STORE:-mayhem-canonical-admin}"
SOURCE_DIR="${MAYHEM_SOURCE_DIR:-/opt/mayhem/source}"
STATE_DIR="${MAYHEM_CADENCE_STATE_DIR:-/opt/mayhem/.mayhem-local/settlement}"

json_field() {
    python3 -c '
import json, sys
value = json.loads(sys.stdin.read())
for part in sys.argv[1].split("."):
    if value is None:
        break
    value = value.get(part) if isinstance(value, dict) else None
print("" if value is None else value)
' "$1"
}

positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]] && \
        python3 -c 'import sys; raise SystemExit(0 if int(sys.argv[1]) <= 9007199254740991 else 1)' "$1"
}

non_negative_integer() {
    [[ "$1" =~ ^(0|[1-9][0-9]*)$ ]] && \
        python3 -c 'import sys; raise SystemExit(0 if int(sys.argv[1]) <= 9007199254740991 else 1)' "$1"
}

validate_execution_context() {
    if [[ "${MAYHEM_PAYOUT_TEST_MODE:-0}" == "1" ]]; then
        local test_root="${MAYHEM_PAYOUT_TEST_ROOT:-}"
        [[ -n "$test_root" && -d "$test_root" ]] || {
            echo "abort: finalizer test mode requires an existing MAYHEM_PAYOUT_TEST_ROOT" >&2
            return 1
        }
        for path in "$MAYHEM_BIN" "$ADMIN_HOME" "$STATE_DIR"; do
            [[ "$path" == "$test_root"/* ]] || {
                echo "abort: finalizer test mode path escapes MAYHEM_PAYOUT_TEST_ROOT: $path" >&2
                return 1
            }
        done
        [[ "$RPC_URL" =~ ^http://127\.0\.0\.1:[1-9][0-9]*/v1$ ]] || {
            echo "abort: finalizer test mode requires an isolated loopback RPC endpoint" >&2
            return 1
        }
        for key in \
            MAYHEM_STRIPE_SECRET_KEY \
            STRIPE_SECRET_KEY \
            MAYHEM_FIAT_STRIPE_ENV_FILE \
            MAYHEM_ADMIN_WALLET_PASSWORD \
            MAYHEM_WALLET_PASSWORD \
            MAYHEM_TAP_ROLLER_PRIVATE_KEY \
            TAP_ROLLER_PRIVATE_KEY \
            MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY \
            TAP_GOVERNANCE_PRIVATE_KEY \
            MAYHEM_TAP_ETH_RPC \
            MAYHEM_TAP_ETH_RPC_FALLBACKS \
            MAYHEM_TNK_TREASURY_KEYPAIR_PATH \
            MAYHEM_TNK_WALLET_PASSWORD; do
            [[ -z "${!key:-}" ]] || {
                echo "abort: finalizer test mode refuses inherited credential $key" >&2
                return 1
            }
        done
        return 0
    fi

    [[ "${MAYHEM_NETWORK:-}" == "mainnet" && "${MAYHEM_MSB_NETWORK:-}" == "mainnet" ]] || {
        echo "abort: Mayhem and MSB networks are not canonical mainnet" >&2
        return 1
    }
    [[ "${MSB_BOOTSTRAP:-}" == "acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685" ]] || {
        echo "abort: MSB_BOOTSTRAP is not the official mainnet bootstrap" >&2
        return 1
    }
    [[ "${MSB_CHANNEL:-}" == "0000trac0network0msb0mainnet0000" ]] || {
        echo "abort: MSB_CHANNEL is not the official mainnet channel" >&2
        return 1
    }
    [[ -f "$SOURCE_DIR/scripts/mainnet-proof.mjs" ]] || {
        echo "abort: canonical mainnet proof helper is missing" >&2
        return 1
    }
    command -v node >/dev/null 2>&1 || {
        echo "abort: node is required for canonical mainnet proof" >&2
        return 1
    }
    if ! node "$SOURCE_DIR/scripts/mainnet-proof.mjs" \
        --peer-rpc "$RPC_URL" --timeout-seconds 0 --json \
        >"$STATE_DIR/finalizer-mainnet-proof.json.tmp"; then
        rm -f "$STATE_DIR/finalizer-mainnet-proof.json.tmp"
        echo "abort: canonical mainnet proof failed; refusing epoch finalization" >&2
        return 1
    fi
    mv "$STATE_DIR/finalizer-mainnet-proof.json.tmp" "$STATE_DIR/finalizer-mainnet-proof.json"
}

bind_epoch_artifact() {
    local epoch="$1" apply_hash="$2" run_dir="$3"
    python3 - "$run_dir/epoch-artifact.json.tmp" "$run_dir/epoch-artifact.json" \
        "$run_dir/epoch-bundle.json" "$run_dir/epoch-recomputed.json" \
        "$run_dir/canonical-receipts.json" \
        "$epoch" "$apply_hash" <<'PY'
import hashlib, json, os, re, sys
target, existing, bundle_path, recomputed_path, receipts_path, epoch, apply_hash = sys.argv[1:]
epoch = int(epoch)

def load(path):
    with open(path, "rb") as source:
        raw = source.read()
    return raw, json.loads(raw)

bundle_raw, bundle = load(bundle_path)
recomputed_raw, recomputed = load(recomputed_path)
receipts_raw, receipts = load(receipts_path)
if bundle.get("epoch") != epoch or recomputed.get("epoch") != epoch:
    raise SystemExit("retained epoch bundle or recomputation has the wrong epoch")
if not isinstance(receipts, dict) or bundle.get("receipt_snapshot") != receipts:
    raise SystemExit("epoch bundle does not match the frozen canonical receipt snapshot")
if receipts.get("settlement_epoch") != epoch or "epoch" in receipts:
    raise SystemExit("frozen canonical receipt snapshot has the wrong settlement epoch")
if bundle.get("receipts") != receipts.get("heads"):
    raise SystemExit("epoch bundle receipt heads do not match the frozen canonical snapshot")
roots = recomputed.get("roots")
if not isinstance(roots, dict) or any(
    not re.fullmatch(r"[0-9a-f]{64}", str(roots.get(key, "")))
    for key in ("dep", "use", "earn", "fee", "price")
):
    raise SystemExit("retained epoch recomputation has invalid roots")
artifact = {
    "schema_version": 1,
    "type": "canonical_epoch_artifact",
    "rail": "all",
    "rails": ["fiat", "tap", "tnk"],
    "epoch": epoch,
    "epoch_apply_hash": apply_hash,
    "bundle_sha256": hashlib.sha256(bundle_raw).hexdigest(),
    "recomputed_sha256": hashlib.sha256(recomputed_raw).hexdigest(),
    "canonical_receipts_sha256": hashlib.sha256(receipts_raw).hexdigest(),
    "roots": roots,
    "totals": recomputed.get("totals"),
}
if os.path.exists(existing) and json.load(open(existing)) != artifact:
    raise SystemExit("existing retained epoch artifact is stale or mismatched")
with open(target, "w") as out:
    json.dump(artifact, out, indent=2)
    out.write("\n")
PY
    mv "$run_dir/epoch-artifact.json.tmp" "$run_dir/epoch-artifact.json"
}

assert_receipt_metadata_unchanged() {
    local epoch="$1" snapshot_path="$2"
    python3 - "$RPC_URL" "$epoch" "$snapshot_path" <<'PY'
import json, sys, urllib.parse, urllib.request
rpc, epoch, snapshot_path = sys.argv[1:]
snapshot = json.load(open(snapshot_path))

def state(key):
    query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
    with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
        record = json.load(response)
    if (
        not isinstance(record, dict)
        or record.get("confirmed") is not True
        or record.get("key") not in (None, key)
    ):
        raise SystemExit(f"canonical state {key} is not confirmed")
    return record.get("value")

metadata = state(f"receipt/epoch/{epoch}/index")
if metadata != snapshot.get("metadata"):
    raise SystemExit("canonical receipt metadata changed after snapshot; retry after the quiet window")
PY
}

read_epoch_commit_hash() {
    local epoch="$1" recomputed_path="$2" at="$3"
    python3 - "$RPC_URL" "$epoch" "$recomputed_path" "$at" <<'PY'
import json, re, sys, urllib.parse, urllib.request
rpc, epoch_text, recomputed_path, at_text = sys.argv[1:]
epoch = int(epoch_text)
at = int(at_text)
key = f"epoch/commit/{epoch}"
query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
    state = json.load(response)
if (
    not isinstance(state, dict)
    or state.get("confirmed") is not True
    or state.get("key") not in (None, key)
):
    raise SystemExit("canonical provisional epoch commit is not confirmed")
value = state.get("value")
recomputed = json.load(open(recomputed_path))
if (
    not isinstance(value, dict)
    or value.get("type") != "epoch_commit"
    or value.get("status") != "provisional"
    or value.get("epoch") != epoch
    or value.get("at") != at
    or value.get("roots") != recomputed.get("roots")
    or value.get("totals") != recomputed.get("totals")
    or not re.fullmatch(r"[0-9a-f]{64}", str(value.get("commit_hash", "")))
):
    raise SystemExit("canonical provisional epoch commit does not match frozen recomputation")
print(value["commit_hash"])
PY
}

write_targeted_feature() {
    local page_path="$1" feature_path="$2"
    node --input-type=module - \
        "$SOURCE_DIR/intercom/scripts/recompute-epoch-roots.mjs" \
        "$page_path" "$feature_path" <<'NODE'
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [modulePath, pagePath, featurePath] = process.argv.slice(2);
const { opaqueHash } = await import(pathToFileURL(modulePath).href);
const page = JSON.parse(fs.readFileSync(pagePath, 'utf8'));
const value = {
  op: 'apply_targeted_epoch',
  epoch: page.epoch,
  at: page.at,
  epoch_commit_hash: page.epoch_commit_hash,
  receipt_index: page.receipt_index,
  debits: page.debits,
  earnings: page.earnings,
  allocations: page.allocations,
  market_usage: page.market_usage,
  page: page.page,
  last_page: page.last_page,
};
const digest = await opaqueHash('mayhem-targeted-epoch-feature-v1', value);
const feature = {
  feature: 'mayhem',
  key: `epoch/targeted/${value.epoch}/${digest}`,
  value,
};
const temporary = `${featurePath}.tmp`;
const fd = fs.openSync(temporary, 'w', 0o600);
try {
  fs.writeFileSync(fd, `${JSON.stringify(feature, null, 2)}\n`);
  fs.fsyncSync(fd);
} finally {
  fs.closeSync(fd);
}
fs.renameSync(temporary, featurePath);
const directory = fs.openSync(path.dirname(featurePath), 'r');
try {
  fs.fsyncSync(directory);
} finally {
  fs.closeSync(directory);
}
NODE
}

wait_for_targeted_page() {
    local epoch="$1" page="$2" last_page="$3"
    python3 - "$RPC_URL" "$epoch" "$page" "$last_page" \
        "${MAYHEM_FINALIZER_STATE_WAIT_SECONDS:-60}" <<'PY'
import json, sys, time, urllib.parse, urllib.request
rpc, epoch_text, page_text, last_text, timeout_text = sys.argv[1:]
epoch = int(epoch_text)
page = int(page_text)
last_page = last_text == "true"
deadline = time.monotonic() + int(timeout_text)
key = "epoch/apply/state"
last = None
while time.monotonic() <= deadline:
    query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
    try:
        with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
            record = json.load(response)
        if (
            isinstance(record, dict)
            and record.get("confirmed") is True
            and record.get("key") in (None, key)
        ):
            last = record.get("value")
            if isinstance(last, dict):
                if last_page:
                    if (
                        last.get("updated_epoch") == epoch
                        and last.get("pending_epoch") is None
                        and last.get("last_page") == page
                    ):
                        raise SystemExit(0)
                elif (
                    last.get("pending_epoch") == epoch
                    and last.get("pending_next_page") == page + 1
                    and last.get("last_page") == page
                ):
                    raise SystemExit(0)
    except (OSError, ValueError):
        pass
    time.sleep(0.2)
raise SystemExit(f"targeted apply page {page} did not become canonical: {last}")
PY
}

mkdir -p "$STATE_DIR/epochs"
if [[ "${MAYHEM_PAYOUT_LOCK_HELD:-0}" != "1" ]]; then
    command -v flock >/dev/null 2>&1 || {
        echo "abort: flock is required for epoch finalization serialization" >&2
        exit 1
    }
    exec 9>"$STATE_DIR/payout.lock"
    if ! flock -n 9; then
        echo "skip: another payout/finalization worker holds $STATE_DIR/payout.lock"
        exit 0
    fi
    export MAYHEM_PAYOUT_LOCK_HELD=1
fi
validate_execution_context

apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
updated_epoch="$(printf '%s' "$apply_state" | json_field value.updated_epoch)"
pending_epoch="$(printf '%s' "$apply_state" | json_field value.pending_epoch)"
pending_next_page="$(printf '%s' "$apply_state" | json_field value.pending_next_page)"
non_negative_integer "$updated_epoch" || {
    echo "abort: canonical epoch/apply/state.updated_epoch is missing or invalid" >&2
    exit 1
}
next_epoch=$((updated_epoch + 1))
epoch="${1:-${pending_epoch:-$next_epoch}}"
positive_integer "$epoch" || {
    echo "abort: epoch must be a positive canonical integer" >&2
    exit 1
}
if [[ -n "$pending_epoch" ]]; then
    [[ "$pending_epoch" == "$next_epoch" && "$epoch" == "$pending_epoch" ]] || {
        echo "abort: pending epoch $pending_epoch does not match requested next epoch $epoch" >&2
        exit 1
    }
    non_negative_integer "$pending_next_page" || {
        echo "abort: pending epoch $pending_epoch has no canonical next page" >&2
        exit 1
    }
fi
if [[ "$epoch" != "$next_epoch" && "$epoch" != "$updated_epoch" ]]; then
    echo "abort: epoch $epoch is neither next nor current (updated_epoch=$updated_epoch)" >&2
    exit 1
fi

run_dir="$STATE_DIR/epochs/epoch-$epoch"
mkdir -p "$run_dir"
echo "settling epoch $epoch; artifacts in $run_dir"

if [[ "$epoch" == "$updated_epoch" ]]; then
    apply_hash="$(printf '%s' "$apply_state" | json_field value.last_apply_hash)"
    [[ "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
        echo "abort: applied epoch $epoch has no canonical apply hash" >&2
        exit 1
    }
    apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"
    if [[ -f "$run_dir/epoch-apply-hash" && \
          "$(tr '[:upper:]' '[:lower:]' <"$run_dir/epoch-apply-hash")" != "$apply_hash" ]]; then
        echo "abort: retained epoch artifacts do not match canonical apply hash" >&2
        exit 1
    fi
    for artifact in epoch-bundle.json epoch-recomputed.json canonical-receipts.json; do
        [[ -f "$run_dir/$artifact" ]] || {
            echo "abort: applied epoch retry is missing $run_dir/$artifact" >&2
            exit 1
        }
    done
    printf '%s\n' "$apply_hash" >"$run_dir/epoch-apply-hash.tmp"
    mv "$run_dir/epoch-apply-hash.tmp" "$run_dir/epoch-apply-hash"
    bind_epoch_artifact "$epoch" "$apply_hash" "$run_dir"
    echo "epoch $epoch was already finalized; canonical evidence was reverified"
    exit 0
fi

[[ ! -f "$run_dir/epoch-artifact.json" && ! -f "$run_dir/epoch-apply-hash" ]] || {
    echo "abort: unapplied epoch directory contains stale apply-bound evidence" >&2
    exit 1
}

if [[ -n "$pending_epoch" ]]; then
    for artifact in \
        finalization-at \
        epoch-bundle.json \
        epoch-recomputed.json \
        canonical-receipts.json; do
        [[ -f "$run_dir/$artifact" ]] || {
            echo "abort: pending paged apply cannot resume without frozen $run_dir/$artifact" >&2
            exit 1
        }
    done
fi

if [[ ! -f "$run_dir/finalization-at" ]]; then
    date +%s >"$run_dir/finalization-at.tmp"
    mv "$run_dir/finalization-at.tmp" "$run_dir/finalization-at"
fi
at="$(cat "$run_dir/finalization-at")"
[[ "$at" =~ ^[1-9][0-9]*$ ]] || {
    echo "abort: invalid frozen finalization timestamp" >&2
    exit 1
}

if [[ ! -f "$run_dir/epoch-bundle.json" ]]; then
    python3 - "$RPC_URL" "$epoch" "$run_dir/canonical-receipts.json" \
        "$run_dir/prior-earnings.json" "$run_dir/epoch-bundle.json" "$at" <<'PY'
import hashlib
import json
import os
import re
import sys
import urllib.parse
import urllib.request

rpc, epoch_text, snapshot_path, prior_path, bundle_path, at_text = sys.argv[1:]
epoch = int(epoch_text)
at = int(at_text)
MAX_PAGE_IDENTITIES = 1000
MAX_RECEIPTS = 1_000_000
HEX_32 = re.compile(r"^[0-9a-f]{64}$")
AU = re.compile(r"^(0|[1-9][0-9]*)$")

def stable_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)

def fsync_json(path, value):
    tmp = f"{path}.tmp"
    data = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "wb", closefd=False) as target:
            target.write(data)
            target.flush()
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(tmp, path)
    directory = os.open(os.path.dirname(path), os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)

def state(key, *, required=True):
    query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
    with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
        record = json.load(response)
    if not isinstance(record, dict):
        raise SystemExit(f"canonical state {key} response is not an object")
    if record.get("key") not in (None, key):
        raise SystemExit(f"canonical state {key} returned a different key")
    if record.get("confirmed") is not True:
        raise SystemExit(f"canonical state {key} is not confirmed")
    value = record.get("value")
    if required and value is None:
        raise SystemExit(f"canonical state {key} is missing")
    return value

def safe_int(value, label, *, allow_zero=True):
    if not isinstance(value, int) or isinstance(value, bool) or value < (0 if allow_zero else 1):
        raise SystemExit(f"{label} is not a canonical integer")
    if value > 9_007_199_254_740_991:
        raise SystemExit(f"{label} exceeds the safe integer range")
    return value

def hex32(value, label):
    if not isinstance(value, str) or not HEX_32.fullmatch(value):
        raise SystemExit(f"{label} must be 32 bytes of lowercase hex")
    return value

def canonical_au(value, label, *, allow_zero=True):
    if not isinstance(value, str) or not AU.fullmatch(value):
        raise SystemExit(f"{label} must be a canonical decimal string")
    if not allow_zero and value == "0":
        raise SystemExit(f"{label} must be positive")
    return value

def active_param(key, default):
    record = state(f"params/{key}", required=False)
    if record is None:
        return default
    if (
        not isinstance(record, dict)
        or record.get("key") not in (None, key)
        or not isinstance(record.get("current"), dict)
        or record.get("pending") is not None and not isinstance(record.get("pending"), dict)
    ):
        raise SystemExit(f"canonical parameter {key} is malformed")
    selected = record["current"]
    pending = record.get("pending")
    if pending is not None:
        effective_at = safe_int(
            pending.get("effective_at"),
            f"canonical parameter {key} pending effective_at",
        )
        if effective_at <= at:
            selected = pending
    safe_int(
        selected.get("effective_at"),
        f"canonical parameter {key} effective_at",
    )
    return selected.get("value")

metadata_key = f"receipt/epoch/{epoch}/index"
metadata = state(metadata_key)
if not isinstance(metadata, dict):
    raise SystemExit("canonical receipt metadata must be an object")
if metadata.get("type") != "canonical_receipt_epoch_index" or metadata.get("epoch") != epoch:
    raise SystemExit("canonical receipt metadata epoch mismatch")
count = safe_int(metadata.get("count"), "canonical receipt count")
page_count = safe_int(metadata.get("page_count"), "canonical receipt page_count")
page_size = safe_int(metadata.get("page_size"), "canonical receipt page_size", allow_zero=False)
safe_int(metadata.get("revision"), "canonical receipt metadata revision")
if not isinstance(metadata.get("updated_at"), str) or not metadata["updated_at"]:
    raise SystemExit("canonical receipt metadata updated_at is invalid")
if page_size > MAX_PAGE_IDENTITIES:
    raise SystemExit("canonical receipt page size exceeds the bounded finalizer limit")
if count > MAX_RECEIPTS:
    raise SystemExit("canonical receipt count exceeds the bounded finalizer limit")
if (count == 0) != (page_count == 0):
    raise SystemExit("canonical receipt count/page_count mismatch")
if count == 0:
    raise SystemExit("canonical non-empty finalizer received an empty receipt epoch")
if page_count > count:
    raise SystemExit("canonical receipt page_count exceeds receipt count")

identities = []
for page in range(page_count):
    page_key = f"receipt/epoch/{epoch}/page/{page}"
    page_value = state(page_key)
    values = page_value.get("identities") if isinstance(page_value, dict) else None
    expected_page_count = min(page_size, count - page * page_size)
    if (
        not isinstance(page_value, dict)
        or page_value.get("type") != "canonical_receipt_epoch_page"
        or page_value.get("epoch") != epoch
        or page_value.get("page") != page
        or not isinstance(values, list)
        or len(values) != expected_page_count
    ):
        raise SystemExit(f"canonical receipt page {page} is empty, malformed, or oversized")
    for identity in values:
        if (
            not isinstance(identity, dict)
            or sorted(identity) != ["billing_attempt", "billing_id"]
        ):
            raise SystemExit("canonical receipt identity has an incompatible shape")
        normalized = {
            "billing_id": hex32(identity.get("billing_id"), "receipt identity billing_id"),
            "billing_attempt": safe_int(
                identity.get("billing_attempt"),
                "receipt identity billing_attempt",
            ),
        }
        identities.append(normalized)

if len(identities) != count:
    raise SystemExit("canonical receipt pages do not match metadata count")
if len({(item["billing_id"], item["billing_attempt"]) for item in identities}) != count:
    raise SystemExit("canonical receipt identities contain a replay")

heads = []
for identity in identities:
    head_key = f"receipt/head/{identity['billing_id']}/{identity['billing_attempt']}"
    head = state(head_key)
    if not isinstance(head, dict):
        raise SystemExit(f"canonical receipt head {head_key} must be an object")
    if head.get("billing_id") != identity["billing_id"] or head.get("billing_attempt") != identity["billing_attempt"]:
        raise SystemExit("canonical receipt head identity mismatch")
    if head.get("epoch") != epoch:
        raise SystemExit("canonical receipt head settlement epoch mismatch")
    billing_epoch = safe_int(
        head.get("billing_epoch"),
        "canonical receipt head billing_epoch",
        allow_zero=False,
    )
    if billing_epoch > epoch:
        raise SystemExit("canonical receipt billing_epoch is after the settlement epoch")
    reservation_id = hex32(head.get("reservation_id"), "canonical receipt head reservation_id")
    payout_revision = hex32(head.get("payout_revision"), "canonical receipt head payout_revision")
    hex32(head.get("receipt_hash"), "canonical receipt head receipt_hash")
    canonical_au(head.get("incremental_au"), "canonical receipt head incremental_au", allow_zero=False)
    receipt = head.get("receipt")
    body = receipt.get("body") if isinstance(receipt, dict) else None
    if not isinstance(body, dict) or body.get("schema_version") != 11:
        raise SystemExit("canonical receipt head must contain a schema v11 signed receipt")
    if (
        body.get("billing_id") != identity["billing_id"]
        or body.get("billing_attempt") != identity["billing_attempt"]
        or body.get("billing_epoch") != billing_epoch
        or body.get("reservation_id") != reservation_id
        or body.get("payout_revision") != payout_revision
    ):
        raise SystemExit("canonical receipt head fields do not match its signed receipt body")
    heads.append(head)

snapshot = {
    "schema_version": 1,
    "type": "canonical_epoch_receipt_snapshot",
    "settlement_epoch": epoch,
    "metadata": metadata,
    "identities": identities,
    "heads": heads,
}
snapshot["snapshot_sha256"] = hashlib.sha256(stable_json(snapshot).encode()).hexdigest()
fsync_json(snapshot_path, snapshot)

pairs = set()
for head in heads:
    body = head["receipt"]["body"]
    rail = body.get("rail")
    provider = body.get("provider")
    if rail not in {"fiat", "tap", "tnk"} or not isinstance(provider, str) or not provider:
        raise SystemExit("canonical signed receipt rail/provider is invalid")
    pairs.add((rail, provider))

prior_earnings = {}
for rail, provider in sorted(pairs):
    record = state(f"earn/{rail}/{provider}", required=False) or {}
    prior_earnings[f"{rail}/{provider}"] = canonical_au(
        record.get("total_au", "0"),
        f"earn/{rail}/{provider}.total_au",
    )
fsync_json(prior_path, prior_earnings)

fee_cum = 0
burn_cum = 0
for rail in ("fiat", "tap", "tnk"):
    fee = state(f"fee/{rail}/cum", required=False) or {}
    burn = state(f"burn/{rail}/cum", required=False) or {}
    fee_cum += int(canonical_au(fee.get("cum_au", "0"), f"fee/{rail}/cum.cum_au"))
    burn_cum += int(canonical_au(burn.get("cum_au", "0"), f"burn/{rail}/cum.cum_au"))

fee_bps = safe_int(active_param("fee_bps", 1500), "canonical fee_bps")
if fee_bps > 1500:
    raise SystemExit("canonical fee_bps exceeds the contract maximum")
max_apply_batch = safe_int(
    active_param("max_apply_batch", 2000),
    "canonical max_apply_batch",
    allow_zero=False,
)
max_market_usage_entries = safe_int(
    active_param("max_market_usage_entries", 5000),
    "canonical max_market_usage_entries",
)
bundle = {
    "epoch": epoch,
    "params": {
        "fee_bps": fee_bps,
        "max_apply_batch": max_apply_batch,
        "max_market_usage_entries": max_market_usage_entries,
    },
    "deposits": [],
    "receipts": heads,
    "receipt_snapshot": snapshot,
    "payouts": [],
    "price_derivations": [],
    "prior_earnings": prior_earnings,
    "prior_fee_cum_au": str(fee_cum),
    "prior_burn_cum_au": str(burn_cum),
}
deposit = state(f"ev/dep/{epoch}", required=False)
if deposit is not None:
    if (
        not isinstance(deposit, dict)
        or not HEX_32.fullmatch(str(deposit.get("merkle_root", "")))
        or safe_int(deposit.get("count"), "canonical deposit count") < 0
    ):
        raise SystemExit("canonical deposit root evidence is malformed")
    bundle["deposit_root"] = {
        "merkle_root": deposit["merkle_root"],
        "count": deposit["count"],
        "au_total": canonical_au(deposit.get("au_total"), "canonical deposit au_total"),
        "source": f"ev/dep/{epoch}",
    }
fsync_json(bundle_path, bundle)
print(
    f"canonical_receipts={count} pages={page_count} fee_bps={fee_bps} "
    f"max_apply_batch={max_apply_batch} "
    f"max_market_usage_entries={max_market_usage_entries}"
)
PY
else
    bundle_epoch="$(json_field epoch <"$run_dir/epoch-bundle.json")"
    [[ "$bundle_epoch" == "$epoch" ]] || {
        echo "abort: frozen bundle epoch $bundle_epoch does not match $epoch" >&2
        exit 1
    }
    echo "reusing frozen canonical receipt bundle for epoch $epoch"
fi

[[ -f "$run_dir/canonical-receipts.json" ]] || {
    echo "abort: frozen epoch bundle is missing its exact canonical receipt snapshot" >&2
    exit 1
}
python3 - "$run_dir/epoch-bundle.json" "$run_dir/canonical-receipts.json" "$epoch" <<'PY'
import json, sys
bundle = json.load(open(sys.argv[1]))
snapshot = json.load(open(sys.argv[2]))
expected_epoch = int(sys.argv[3])
if bundle.get("epoch") != expected_epoch:
    raise SystemExit("frozen receipt bundle epoch is stale")
receipts = bundle.get("receipts")
if bundle.get("receipt_snapshot") != snapshot or receipts != snapshot.get("heads"):
    raise SystemExit("frozen receipt bundle does not match its exact canonical snapshot")
PY

node "$SOURCE_DIR/intercom/scripts/recompute-epoch-roots.mjs" "$run_dir/epoch-bundle.json" \
    >"$run_dir/epoch-recomputed.json.tmp"
if [[ -f "$run_dir/epoch-recomputed.json" ]]; then
    if ! cmp -s "$run_dir/epoch-recomputed.json" "$run_dir/epoch-recomputed.json.tmp"; then
        rm -f "$run_dir/epoch-recomputed.json.tmp"
        echo "abort: deterministic recomputation changed for the frozen epoch snapshot" >&2
        exit 1
    fi
    rm -f "$run_dir/epoch-recomputed.json.tmp"
else
    mv "$run_dir/epoch-recomputed.json.tmp" "$run_dir/epoch-recomputed.json"
fi
echo "recomputed: use_au=$(json_field totals.use_au <"$run_dir/epoch-recomputed.json") earn_au=$(json_field totals.earn_au <"$run_dir/epoch-recomputed.json")"

commit_common=(
    admin epoch-commit
    --home "$ADMIN_HOME"
    --rpc-url "$RPC_URL"
    --peer-store-name "$ADMIN_STORE"
    --recomputed-file "$run_dir/epoch-recomputed.json"
    --at "$at"
    --submit --json
)
if [[ -z "$pending_epoch" ]]; then
    assert_receipt_metadata_unchanged "$epoch" "$run_dir/canonical-receipts.json"
    "$MAYHEM_BIN" "${commit_common[@]}" --sim >"$run_dir/epoch-commit-sim.json"
    sim_ok="$(python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
ok = d.get("ok")
if ok is None:
    ok = d.get("tx", {}).get("result", {}).get("ok")
print(ok)
' "$run_dir/epoch-commit-sim.json")"
    if [[ "$sim_ok" != "True" && "$sim_ok" != "true" ]]; then
        echo "abort: epoch-commit sim did not return ok" >&2
        exit 1
    fi
    assert_receipt_metadata_unchanged "$epoch" "$run_dir/canonical-receipts.json"
    "$MAYHEM_BIN" "${commit_common[@]}" >"$run_dir/epoch-commit.json"
    echo "epoch-commit submitted"
else
    echo "resuming pending targeted apply at page $pending_next_page"
fi

assert_receipt_metadata_unchanged "$epoch" "$run_dir/canonical-receipts.json"
commit_hash="$(read_epoch_commit_hash "$epoch" "$run_dir/epoch-recomputed.json" "$at")"
[[ "$commit_hash" =~ ^[0-9a-f]{64}$ ]] || {
    echo "abort: canonical provisional epoch commit hash is invalid" >&2
    exit 1
}

pages_dir="$run_dir/apply-pages"
mkdir -p "$pages_dir"
page_count="$(python3 - "$run_dir/epoch-recomputed.json" \
    "$run_dir/canonical-receipts.json" "$pages_dir" \
    "$epoch" "$at" "$commit_hash" <<'PY'
import glob, hashlib, json, os, re, sys
recomputed_path, snapshot_path, pages_dir, epoch_text, at_text, commit_hash = sys.argv[1:]
epoch = int(epoch_text)
at = int(at_text)

def stable_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)

def canonical_au(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"(0|[1-9][0-9]*)", value):
        raise SystemExit(f"{label} is not canonical au")
    return int(value)

def fsync_json(path, value):
    data = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()
    if os.path.exists(path):
        if open(path, "rb").read() != data:
            raise SystemExit(f"retained targeted apply page changed: {path}")
        return
    tmp = f"{path}.tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "wb", closefd=False) as target:
            target.write(data)
            target.flush()
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(tmp, path)
    directory = os.open(os.path.dirname(path), os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)

recomputed = json.load(open(recomputed_path))
snapshot = json.load(open(snapshot_path))
receipt_index = snapshot.get("metadata")
pages = recomputed.get("apply_pages")
if (
    recomputed.get("epoch") != epoch
    or recomputed.get("receipt_index") != receipt_index
    or not isinstance(pages, list)
    or not pages
    or not re.fullmatch(r"[0-9a-f]{64}", commit_hash)
):
    raise SystemExit("recomputed targeted apply pages do not match the frozen epoch")
expected_count = receipt_index.get("count") if isinstance(receipt_index, dict) else None
if not isinstance(expected_count, int) or isinstance(expected_count, bool) or expected_count < 1:
    raise SystemExit("frozen receipt index count is invalid")

cumulative = 0
for number, page in enumerate(pages):
    if (
        not isinstance(page, dict)
        or page.get("page") != number
        or page.get("receipt_index") != receipt_index
        or not isinstance(page.get("allocations"), list)
        or not page["allocations"]
        or not isinstance(page.get("debits"), list)
        or not isinstance(page.get("earnings"), list)
        or not isinstance(page.get("market_usage"), list)
        or not isinstance(page.get("last_page"), bool)
    ):
        raise SystemExit(f"recomputed targeted apply page {number} is malformed")
    for allocation in page["allocations"]:
        billing_epoch = allocation.get("billing_epoch") if isinstance(allocation, dict) else None
        if (
            not isinstance(billing_epoch, int)
            or isinstance(billing_epoch, bool)
            or billing_epoch < 1
            or billing_epoch > epoch
        ):
            raise SystemExit(
                f"targeted apply page {number} allocation has an invalid billing_epoch"
            )
    page_hash = page.get("page_sha256")
    hash_value = {key: value for key, value in page.items() if key != "page_sha256"}
    if (
        not isinstance(page_hash, str)
        or hashlib.sha256(stable_json(hash_value).encode()).hexdigest() != page_hash
    ):
        raise SystemExit(f"recomputed targeted apply page {number} hash mismatch")
    allocation_total = sum(
        canonical_au(entry.get("au"), f"page {number} allocation au")
        for entry in page["allocations"]
    )
    debit_total = sum(
        canonical_au(entry.get("au"), f"page {number} debit au")
        for entry in page["debits"]
    )
    earning_total = sum(
        canonical_au(entry.get("gross_au"), f"page {number} earning au")
        for entry in page["earnings"]
    )
    market_total = sum(
        canonical_au(entry.get("demand_au"), f"page {number} market usage au")
        for entry in page["market_usage"]
    )
    if not (allocation_total == debit_total == earning_total == market_total):
        raise SystemExit(f"targeted apply page {number} aggregates do not reconcile")
    cumulative += len(page["allocations"])
    expected_last = cumulative == expected_count
    if page["last_page"] is not expected_last:
        raise SystemExit(f"targeted apply page {number} last_page is inconsistent")
    submission = {
        "schema_version": 1,
        "type": "canonical_targeted_epoch_apply_page",
        "epoch": epoch,
        "at": at,
        "epoch_commit_hash": commit_hash,
        "receipt_index": receipt_index,
        "page": number,
        "last_page": page["last_page"],
        "allocations": page["allocations"],
        "debits": page["debits"],
        "earnings": page["earnings"],
        "market_usage": page["market_usage"],
        "page_sha256": page_hash,
    }
    submission["submission_sha256"] = hashlib.sha256(stable_json(submission).encode()).hexdigest()
    fsync_json(os.path.join(pages_dir, f"page-{number}.json"), submission)

if cumulative != expected_count or pages[-1].get("last_page") is not True:
    raise SystemExit("targeted apply pages do not cover the frozen receipt index")
expected_paths = {
    os.path.join(pages_dir, f"page-{number}.json")
    for number in range(len(pages))
}
retained_page_paths = {
    candidate
    for candidate in glob.glob(os.path.join(pages_dir, "page-*.json"))
    if re.fullmatch(r"page-[0-9]+\.json", os.path.basename(candidate))
}
unexpected = retained_page_paths - expected_paths
if unexpected:
    raise SystemExit("retained apply page directory contains unexpected pages")
print(len(pages))
PY
)" || {
    echo "abort: failed to materialize bounded targeted apply pages" >&2
    exit 1
}
positive_integer "$page_count" || {
    echo "abort: bounded targeted apply page count is invalid" >&2
    exit 1
}

current_apply_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state&confirmed=true")"
current_updated_epoch="$(printf '%s' "$current_apply_state" | json_field value.updated_epoch)"
current_pending_epoch="$(printf '%s' "$current_apply_state" | json_field value.pending_epoch)"
current_pending_page="$(printf '%s' "$current_apply_state" | json_field value.pending_next_page)"
if [[ "$current_updated_epoch" == "$epoch" && -z "$current_pending_epoch" ]]; then
    start_page="$page_count"
elif [[ "$current_pending_epoch" == "$epoch" ]]; then
    start_page="$current_pending_page"
else
    start_page=0
fi
non_negative_integer "$start_page" || {
    echo "abort: canonical targeted apply resume page is invalid" >&2
    exit 1
}
(( start_page <= page_count )) || {
    echo "abort: canonical targeted apply resume page exceeds frozen page count" >&2
    exit 1
}

for ((page = start_page; page < page_count; page++)); do
    page_path="$pages_dir/page-$page.json"
    last_page="$(json_field last_page <"$page_path")"
    [[ "$last_page" == "True" || "$last_page" == "False" ]] || {
        echo "abort: targeted apply page $page has invalid last_page" >&2
        exit 1
    }
    last_page_json=false
    [[ "$last_page" == "True" ]] && last_page_json=true

    assert_receipt_metadata_unchanged "$epoch" "$run_dir/canonical-receipts.json"
    feature_path="$pages_dir/page-$page-feature.json"
    write_targeted_feature "$page_path" "$feature_path.candidate"
    if [[ -f "$feature_path" ]]; then
        cmp -s "$feature_path" "$feature_path.candidate" || {
            rm -f "$feature_path.candidate"
            echo "abort: targeted apply feature changed on retry for page $page" >&2
            exit 1
        }
        rm -f "$feature_path.candidate"
    else
        mv "$feature_path.candidate" "$feature_path"
    fi
    cp "$feature_path" "$pages_dir/page-$page-sim.json"

    assert_receipt_metadata_unchanged "$epoch" "$run_dir/canonical-receipts.json"
    response_tmp="$pages_dir/page-$page-submit.json.tmp"
    if ! curl -sf -m 30 -X POST "$RPC_URL/contract/feature" \
        -H 'content-type: application/json' \
        --data-binary "@$feature_path" >"$response_tmp"; then
        rm -f "$response_tmp"
        echo "abort: targeted apply page $page submission failed; retry resumes this exact page" >&2
        exit 1
    fi
    mv "$response_tmp" "$pages_dir/page-$page-submit.json"
    submit_ok="$(json_field ok <"$pages_dir/page-$page-submit.json")"
    if [[ "$submit_ok" != "True" && "$submit_ok" != "true" ]]; then
        submit_error="$(json_field error <"$pages_dir/page-$page-submit.json")"
        echo "abort: targeted apply page $page was rejected${submit_error:+: $submit_error}" >&2
        exit 1
    fi
    wait_for_targeted_page "$epoch" "$page" "$last_page_json"
    echo "targeted epoch apply page $page submitted"
done

sleep "${MAYHEM_FINALIZER_CONFIRM_DELAY_SECONDS:-3}"
after_state="$(curl -sf -m 10 "$RPC_URL/state?key=epoch/apply/state")"
after="$(printf '%s' "$after_state" | json_field value.updated_epoch)"
if [[ "$after" != "$epoch" ]]; then
    echo "abort: applied epoch $epoch but updated_epoch is $after" >&2
    exit 1
fi
apply_hash="$(printf '%s' "$after_state" | json_field value.last_apply_hash)"
[[ "$apply_hash" =~ ^[0-9a-fA-F]{64}$ ]] || {
    echo "abort: applied epoch $epoch did not expose a canonical apply hash" >&2
    exit 1
}
apply_hash="$(printf '%s' "$apply_hash" | tr '[:upper:]' '[:lower:]')"
printf '%s\n' "$apply_hash" >"$run_dir/epoch-apply-hash.tmp"
mv "$run_dir/epoch-apply-hash.tmp" "$run_dir/epoch-apply-hash"
bind_epoch_artifact "$epoch" "$apply_hash" "$run_dir"

echo "epoch $epoch settled; billing epoch is now $((epoch + 1))"
