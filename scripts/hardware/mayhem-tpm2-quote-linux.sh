#!/bin/sh
set -eu

fail() {
  printf '%s\n' "mayhem-tpm2-quote-linux: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

binding="${MAYHEM_HW_QUOTE_BINDING:-${MAYHEM_HW_QUOTE_NONCE:-}}"
[ -n "$binding" ] || fail "MAYHEM_HW_QUOTE_BINDING is required"
case "$binding" in
  *[!0123456789abcdefABCDEF]*)
    fail "MAYHEM_HW_QUOTE_BINDING must be hex"
    ;;
esac
[ "${#binding}" -eq 64 ] || fail "MAYHEM_HW_QUOTE_BINDING must be a 32-byte hex digest"

need tpm2_createek
need tpm2_createak
need tpm2_quote
need sha256sum
need base64
need python3

pcr_selection="${MAYHEM_TPM2_PCR_SELECTION:-sha256:0,2,4,7}"
hash_alg="${MAYHEM_TPM2_HASH_ALG:-sha256}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-tpm2-quote.XXXXXX")"
cleanup() {
  if [ "${use_sudo:-0}" = "1" ]; then
    sudo -n rm -rf "$tmp" >/dev/null 2>&1 || rm -rf "$tmp"
  else
    rm -rf "$tmp"
  fi
}
trap cleanup EXIT INT TERM

use_sudo=0
if [ "${MAYHEM_TPM2_USE_SUDO:-}" = "1" ]; then
  use_sudo=1
elif [ -e /dev/tpmrm0 ] && { [ ! -r /dev/tpmrm0 ] || [ ! -w /dev/tpmrm0 ]; }; then
  if sudo -n true >/dev/null 2>&1; then
    use_sudo=1
  else
    fail "/dev/tpmrm0 is not accessible; add the user to the tss group or set up passwordless sudo for tpm2-tools"
  fi
fi

tpm() {
  if [ "$use_sudo" = "1" ]; then
    sudo -n "$@"
  else
    "$@"
  fi
}

cd "$tmp"
tpm tpm2_createek -G rsa -c ek.ctx -u ek.pub >/dev/null
tpm tpm2_createak -C ek.ctx -G rsa -g "$hash_alg" -s rsassa -c ak.ctx -u ak.pub -n ak.name >/dev/null
tpm tpm2_quote -c ak.ctx -l "$pcr_selection" -q "$binding" -m quote.msg -s quote.sig -o quote.pcrs -g "$hash_alg" >/dev/null

ek_cert_index=""
for index in 0x01c00002 0x01c0000a; do
  if tpm tpm2_nvread -C o "$index" -o ek-cert.der >/dev/null 2>&1 && [ -s ek-cert.der ]; then
    ek_cert_index="$index"
    break
  fi
done

if [ "$use_sudo" = "1" ]; then
  sudo -n chown -R "$(id -u):$(id -g)" "$tmp"
fi

export MAYHEM_TPM2_EVIDENCE_DIR="$tmp"
export MAYHEM_TPM2_BINDING="$binding"
export MAYHEM_TPM2_PCR_SELECTION="$pcr_selection"
export MAYHEM_TPM2_HASH_ALG="$hash_alg"
export MAYHEM_TPM2_EK_CERT_INDEX="$ek_cert_index"

python3 - <<'PY'
import base64
import hashlib
import json
import os
from pathlib import Path

root = Path(os.environ["MAYHEM_TPM2_EVIDENCE_DIR"])

def b64(name):
    return base64.b64encode((root / name).read_bytes()).decode("ascii")

ek_pub = (root / "ek.pub").read_bytes()
device_key = hashlib.sha256(ek_pub).hexdigest()
evidence = {
    "schema_version": 1,
    "kind": "tpm2_quote_ek",
    "binding": os.environ["MAYHEM_TPM2_BINDING"].lower(),
    "pcr_selection": os.environ["MAYHEM_TPM2_PCR_SELECTION"],
    "hash_algorithm": os.environ["MAYHEM_TPM2_HASH_ALG"],
    "tool": "tpm2-tools",
    "ek_public_tss_b64": base64.b64encode(ek_pub).decode("ascii"),
    "ak_public_tss_b64": b64("ak.pub"),
    "ak_name_b64": b64("ak.name"),
    "quote_message_b64": b64("quote.msg"),
    "quote_signature_b64": b64("quote.sig"),
    "quote_pcrs_b64": b64("quote.pcrs"),
    "device_key": device_key,
}
ek_cert = root / "ek-cert.der"
if ek_cert.exists() and ek_cert.stat().st_size:
    evidence["ek_cert_der_b64"] = b64("ek-cert.der")
    evidence["ek_cert_nv_index"] = os.environ.get("MAYHEM_TPM2_EK_CERT_INDEX") or None

print(json.dumps({
    "kind": "tpm2_quote_ek",
    "binding": os.environ["MAYHEM_TPM2_BINDING"].lower(),
    "evidence": json.dumps(evidence, sort_keys=True, separators=(",", ":")),
    "device_key": device_key,
    "tpm": {
        "ek_sha256": device_key,
        "pcr_selection": os.environ["MAYHEM_TPM2_PCR_SELECTION"],
        "ek_cert_present": "ek_cert_der_b64" in evidence,
    },
}, sort_keys=True, separators=(",", ":")))
PY
