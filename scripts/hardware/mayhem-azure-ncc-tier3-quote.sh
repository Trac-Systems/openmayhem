#!/bin/sh
set -eu

fail() {
  printf '%s\n' "mayhem-azure-ncc-tier3-quote: $*" >&2
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
binding="$(printf '%s' "$binding" | tr 'A-F' 'a-f')"

kind="${MAYHEM_HW_QUOTE_KIND:-nvidia_nvtrust_offline_jwt}"
[ "$kind" = "nvidia_nvtrust_offline_jwt" ] || fail "expected nvidia_nvtrust_offline_jwt quote kind, got $kind"

platform_id="${MAYHEM_AZURE_NCC_PLATFORM_ID:-azure-ncc}"
gpu_attestation="${MAYHEM_AZURE_NCC_GPU_ATTESTATION_BIN:-gpu-attestation}"
cpu_attestation="${MAYHEM_AZURE_NCC_CPU_ATTESTATION_BIN:-/usr/local/lib/cvm-attestation/attest}"
cpu_attestation_url="${MAYHEM_AZURE_NCC_MAA_URL:-https://sharedweu.weu.attest.azure.net/attest/SevSnpVm?api-version=2022-08-01}"

need python3
need nvidia-smi
need tpm2_pcrread
need curl
command -v "$gpu_attestation" >/dev/null 2>&1 || fail "missing GPU attestation command: $gpu_attestation"
[ -x "$cpu_attestation" ] || fail "missing CPU attestation command: $cpu_attestation"

run_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  else
    sudo -n "$@"
  fi
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-azure-ncc-tier3.XXXXXX")"
cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

gpu_log="$tmp/gpu-attestation.log"
cpu_log="$tmp/cpu-attestation.log"
cpu_config="$tmp/config_snp.json"
cc_state="$tmp/cc-state.txt"
nvidia_smi_q="$tmp/nvidia-smi-q.txt"
pcrs="$tmp/pcrs.txt"
gpu_query="$tmp/gpu-query.csv"
region_file="$tmp/region.txt"

python3 - "$cpu_config" "$binding" "$cpu_attestation_url" <<'PY'
import json
import sys

config_path, binding, attestation_url = sys.argv[1:4]
config = {
    "attestation_url": attestation_url,
    "attestation_provider": "maa_snp",
    "api_key": "",
    "enable_metrics": False,
    "claims": {"user-claims": {"nonce": binding}},
}
with open(config_path, "w", encoding="utf-8") as fh:
    json.dump(config, fh)
PY
chmod 0644 "$cpu_config"

run_root "$gpu_attestation" --nonce "$binding" --claims_version 3.0 >"$gpu_log" 2>&1
grep -q "GPU Attestation is Successful" "$gpu_log" || fail "GPU attestation did not report success"
grep -qi "nonce.*matching" "$gpu_log" || fail "GPU attestation did not report nonce matching"
grep -qi "$binding" "$gpu_log" || fail "GPU attestation output did not contain the Mayhem binding"

( cd "$tmp" && run_root "$cpu_attestation" --c "$cpu_config" --s >"$cpu_log" 2>&1 )
grep -q "Attested Platform Successfully" "$cpu_log" || fail "CPU/CVM attestation did not report success"

{
  nvidia-smi conf-compute -f
  nvidia-smi conf-compute -e
  nvidia-smi conf-compute -grs
} >"$cc_state" 2>&1
grep -q "CC status: ON" "$cc_state" || fail "GPU CC status is not ON"
grep -q "CC Environment: PRODUCTION" "$cc_state" || fail "GPU CC environment is not PRODUCTION"
grep -qi "ready" "$cc_state" || fail "GPU CC ready state is not ready"

nvidia-smi -q >"$nvidia_smi_q" 2>&1
nvidia-smi --query-gpu=name,driver_version,vbios_version,uuid --format=csv,noheader,nounits >"$gpu_query" 2>&1
run_root tpm2_pcrread >"$pcrs" 2>&1
curl -fsS -H Metadata:true 'http://169.254.169.254/metadata/instance/compute/location?api-version=2021-01-01&format=text' >"$region_file" 2>/dev/null || true

python3 - "$tmp" "$binding" "$kind" "$platform_id" <<'PY'
import base64
import hashlib
import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
binding = sys.argv[2]
kind = sys.argv[3]
platform_id = sys.argv[4]

def text(name):
    return (root / name).read_text(encoding="utf-8", errors="replace")

def b64_or_none(name):
    path = root / name
    if not path.exists():
        return None
    return base64.b64encode(path.read_bytes()).decode("ascii")

gpu_query = text("gpu-query.csv").strip().split(",", 3)
gpu_model = gpu_query[0].strip() if len(gpu_query) > 0 else ""
gpu_driver = gpu_query[1].strip() if len(gpu_query) > 1 else ""
gpu_vbios = gpu_query[2].strip() if len(gpu_query) > 2 else ""
gpu_uuid = gpu_query[3].strip() if len(gpu_query) > 3 else ""
cpu_log = text("cpu-attestation.log")
gpu_log = text("gpu-attestation.log")
user_claims = {"user-claims": {"nonce": binding}}
user_claims_sha512 = hashlib.sha512(json.dumps(user_claims).encode("utf-8")).hexdigest().upper()

def first(pattern, source):
    match = re.search(pattern, source)
    return match.group(1) if match else None

evidence = {
    "schema_version": 1,
    "kind": kind,
    "binding": binding,
    "platform_id": platform_id,
    "region": text("region.txt").strip() or None,
    "cpu": {
        "attestation_tool": "azure-cvm-attestation",
        "maa_url": None,
        "user_claims_sha512": user_claims_sha512,
        "log": cpu_log,
        "snp_report_b64": b64_or_none("report.bin"),
        "runtime_data_json": json.loads(text("runtime_data.json")) if (root / "runtime_data.json").exists() else None,
        "snp_chip_family": first(r"SNP Chip Family:\s*([A-Za-z0-9_.:-]+)", cpu_log) or "Genoa",
        "snp_tcb": first(r"Current TCB version:\s*([A-Fa-f0-9]+)", cpu_log),
        "snp_firmware_svn": first(r"SNP Firmware SVN:\s*([0-9]+)", cpu_log),
    },
    "gpu": {
        "attestation_tool": "nvidia-local-gpu-verifier",
        "log": gpu_log,
        "model": gpu_model,
        "driver": gpu_driver,
        "vbios": gpu_vbios,
        "uuid": gpu_uuid,
        "local_hs256_summary_seen_not_trusted": "HS256" in gpu_log,
    },
    "cc_state": text("cc-state.txt"),
    "nvidia_smi_q": text("nvidia-smi-q.txt"),
    "pcrs": text("pcrs.txt"),
}
print(json.dumps({
    "kind": kind,
    "binding": binding,
    "platform_id": platform_id,
    "region": evidence["region"],
    "snp_chip_family": evidence["cpu"]["snp_chip_family"],
    "snp_tcb": evidence["cpu"]["snp_tcb"],
    "snp_firmware_svn": evidence["cpu"]["snp_firmware_svn"],
    "gpu_model": gpu_model,
    "gpu_driver": gpu_driver,
    "gpu_vbios": gpu_vbios,
    "metadata": {
        "platform_id": platform_id,
        "region": evidence["region"],
        "snp": evidence["cpu"],
        "gpu": evidence["gpu"],
    },
    "evidence": json.dumps(evidence, sort_keys=True, separators=(",", ":")),
}, sort_keys=True))
PY
